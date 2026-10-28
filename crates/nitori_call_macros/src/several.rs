//! `#[call]` for functions whose leading parameters are several accesses.
use crate::{
    AccessBounds, Module, Options, RewriteYield, arguments, camel_case, collect_names, mentions,
    name_impl_traits, name_lifetimes, runtime, split_generics, take_access_bounds,
};
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::{
    Error, FnArg, GenericParam, Generics, Ident, ItemFn, LifetimeParam, Result, ReturnType, Type,
    TypeParamBound, WherePredicate, parse_quote,
    punctuated::Punctuated,
    spanned::Spanned,
    visit_mut::{self, VisitMut},
};

/// How many leading parameters of `item` are accesses.
pub(crate) fn access_count(item: &ItemFn) -> usize {
    let mut generics = item.sig.generics.clone();
    item.sig
        .inputs
        .iter()
        .take_while(|input| match input {
            FnArg::Typed(typed) => take_access_bounds(&typed.ty, &mut generics).is_ok(),
            FnArg::Receiver(_) => false,
        })
        .count()
}

/// Replaces `A::Family` and `<A as ReceiverAccess>::Family` with a family
/// parameter, keeping any associated type that follows.
struct AccessAsFamily<'a> {
    access: &'a Ident,
    family: &'a Ident,
}
impl VisitMut for AccessAsFamily<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(path) = ty {
            let segments: Vec<_> = path.path.segments.iter().cloned().collect();
            let family = self.family;
            if path.qself.is_none()
                && segments.len() >= 2
                && segments[0].ident == *self.access
                && segments[0].arguments.is_empty()
                && segments[1].ident == "Family"
            {
                let rest = &segments[2..];
                *ty = parse_quote!(#family #(::#rest)*);
                return;
            }
            if let Some(qself) = &path.qself
                && crate::is_ident(&qself.ty, self.access)
                && qself.position + 1 == segments.len()
                && segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Family")
            {
                *ty = parse_quote!(#family);
                return;
            }
        }
        visit_mut::visit_type_mut(self, ty);
    }
}

/// One access parameter.
struct Access {
    /// Its type parameter, if it has one.
    name: Option<Ident>,
    bounds: Punctuated<TypeParamBound, syn::Token![+]>,
    /// The family's parameter in generated impls.
    family: Ident,
    /// The access's parameter in generated impls.
    parameter: Ident,
    /// The local holding it in the operation's future.
    local: Ident,
}

pub(crate) fn expand(mut item: ItemFn, options: Options, count: usize) -> Result<Tokens> {
    let runtime = runtime();
    let emitter = Ident::new("__emitter", Span::mixed_site());
    match &options.yields {
        Some(_) => RewriteYield { emitter: &emitter }.visit_block_mut(&mut item.block),
        None => {
            let mut find = crate::FindYield(None);
            syn::visit::Visit::visit_block(&mut find, &item.block);
            if let Some(span) = find.0 {
                return Err(Error::new(
                    span,
                    "declare the item type of `yield` with `#[call(yields = Type)]`",
                ));
            }
        }
    }
    if !matches!(options.module, Module::Default | Module::Current) {
        return Err(Error::new(
            item.sig.span(),
            "an operation over several accesses is generated in the current module",
        ));
    }
    let signature = item.sig.clone();
    if signature.asyncness.is_none() {
        return Err(Error::new(
            signature.span(),
            "#[call] applies to async functions",
        ));
    }

    // The accesses, removed from the generics.
    let mut generics = signature.generics.clone();
    let mut accesses = Vec::new();
    for (index, input) in signature.inputs.iter().take(count).enumerate() {
        let FnArg::Typed(typed) = input else {
            unreachable!()
        };
        let AccessBounds {
            access,
            bounds,
            family,
        } = take_access_bounds(&typed.ty, &mut generics)?;
        accesses.push(Access {
            name: access,
            bounds,
            // A named family keeps its name, so that `F::Name` resolves.
            family: family.unwrap_or_else(|| format_ident!("__F{index}")),
            parameter: format_ident!("__A{index}"),
            local: format_ident!("__io{index}"),
        });
    }
    let mut arguments = arguments(signature.inputs.iter().skip(count), count)?;
    let mut output: Type = match &signature.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };
    let mut yield_type = options.yields.clone();
    // Name each access's family by its parameter.
    for access in &accesses {
        let Some(name) = &access.name else { continue };
        let mut replace = AccessAsFamily {
            access: name,
            family: &access.family,
        };
        replace.visit_type_mut(&mut output);
        if let Some(item) = &mut yield_type {
            replace.visit_type_mut(item);
        }
        if let Some(clause) = &mut generics.where_clause {
            for predicate in clause.predicates.iter_mut() {
                replace.visit_where_predicate_mut(predicate);
            }
        }
        let rest = &generics.where_clause;
        let item = &yield_type;
        for (tokens, what) in [
            (quote!(#output), "the output"),
            (quote!(#item), "the item type"),
            (quote!(#rest), "the where clause"),
        ] {
            if mentions(tokens, &name.to_string()) {
                return Err(Error::new(
                    name.span(),
                    format!("{what} may mention the access only as `{name}::Family`"),
                ));
            }
        }
    }
    for argument in &arguments {
        let ty = &argument.ty;
        let mut names = Vec::new();
        collect_names(quote!(#ty), &mut names);
        let mentions_access = accesses.iter().any(|access| {
            access
                .name
                .iter()
                .chain([&access.family])
                .any(|name| names.contains(&name.to_string()))
        });
        if mentions_access || names.iter().any(|name| name == "ReceiverAccess") {
            return Err(Error::new(
                ty.span(),
                "with several accesses, the other arguments may not depend on them",
            ));
        }
    }

    // Lifetimes belong to the struct; argument-position `impl Trait` becomes
    // a parameter of the struct and of the host method.
    let declared: Vec<LifetimeParam> = generics.lifetimes().cloned().collect();
    generics.params = generics
        .params
        .into_iter()
        .filter(|parameter| !matches!(parameter, GenericParam::Lifetime(_)))
        .collect();
    let lifetimes = name_lifetimes(&mut arguments, &declared, quote!(#generics #output));
    let anonymous = name_impl_traits(&mut arguments);
    let mut all_bounds = Punctuated::<TypeParamBound, syn::Token![+]>::new();
    for access in &accesses {
        all_bounds.extend(access.bounds.iter().cloned());
    }
    // Explicit arguments for calling the function from the operation's future.
    let turbofish: Vec<Tokens> = signature
        .generics
        .params
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) => Some(
                match accesses
                    .iter()
                    .find(|access| access.name.as_ref() == Some(&parameter.ident))
                {
                    Some(access) => {
                        let parameter = &access.parameter;
                        quote!(#parameter)
                    }
                    None => {
                        let ident = &parameter.ident;
                        quote!(#ident)
                    }
                },
            ),
            GenericParam::Const(parameter) => {
                let ident = &parameter.ident;
                Some(quote!(#ident))
            }
            GenericParam::Lifetime(_) => None,
        })
        .collect();
    let turbofish = (!turbofish.is_empty()).then(|| quote!(::<#(#turbofish),*>));
    let (trait_generics, mut method_generics) = split_generics(generics.clone(), &all_bounds);
    method_generics
        .params
        .extend(anonymous.iter().cloned().map(GenericParam::Type));

    let vis = &item.vis;
    let function = &signature.ident;
    let name = format_ident!(
        "{}",
        camel_case(&function.to_string()),
        span = signature.ident.span()
    );
    let names: Vec<&Ident> = arguments.iter().map(|argument| &argument.name).collect();
    let types: Vec<&Type> = arguments.iter().map(|argument| &argument.ty).collect();
    let lifetime_names: Vec<_> = lifetimes
        .iter()
        .map(|parameter| &parameter.lifetime)
        .collect();
    let values: Vec<GenericParam> = generics
        .params
        .iter()
        .cloned()
        .chain(anonymous.iter().cloned().map(GenericParam::Type))
        .collect();
    let value_names: Vec<Ident> = values
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) => Some(parameter.ident.clone()),
            GenericParam::Const(parameter) => Some(parameter.ident.clone()),
            GenericParam::Lifetime(_) => None,
        })
        .collect();
    let marker_types: Vec<&Ident> = values
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) => Some(&parameter.ident),
            _ => None,
        })
        .collect();
    let parameters = quote!(#(#lifetimes,)* #(#values,)*);
    let struct_arguments = quote!(#(#lifetime_names,)* #(#value_names,)*);
    let all_predicates: Vec<WherePredicate> = generics
        .where_clause
        .iter()
        .flat_map(|clause| clause.predicates.iter().cloned())
        .collect();
    let family_names: Vec<String> = accesses
        .iter()
        .map(|access| access.family.to_string())
        .collect();
    let struct_predicates: Vec<&WherePredicate> = all_predicates
        .iter()
        .filter(|predicate| {
            let mut mentioned = Vec::new();
            collect_names(quote!(#predicate), &mut mentioned);
            !mentioned.iter().any(|name| family_names.contains(name))
        })
        .collect();
    let (marker, marker_init) = if lifetime_names.is_empty() && marker_types.is_empty() {
        (Tokens::new(), Tokens::new())
    } else {
        (
            quote! {
                #[doc(hidden)]
                #vis __marker: ::core::marker::PhantomData<fn() -> (#(&#lifetime_names (),)* #(*const #marker_types,)*)>,
            },
            quote!(__marker: ::core::marker::PhantomData,),
        )
    };
    let families: Vec<&Ident> = accesses.iter().map(|access| &access.family).collect();
    let access_parameters: Vec<&Ident> = accesses.iter().map(|access| &access.parameter).collect();
    let locals: Vec<&Ident> = accesses.iter().map(|access| &access.local).collect();
    let family_bounds: Vec<Tokens> = accesses
        .iter()
        .map(|access| {
            let family = &access.family;
            let bounds = access.bounds.iter();
            quote!(#family: #runtime::ReceiverFamily #(+ #bounds)*)
        })
        .collect();
    let receivers = quote!((#(#families,)*));
    let yield_tokens = match &yield_type {
        Some(item) => quote!(#item),
        None => quote!(::core::convert::Infallible),
    };
    let emitter_argument = yield_type.as_ref().map(|_| quote!(&mut __emitter));
    let doc = format!("The `{function}` operation.");

    let definition = quote! {
        #[doc = #doc]
        #[must_use = "operations do nothing until started and polled"]
        #vis struct #name<#parameters>
        where #(#struct_predicates,)*
        {
            #(#vis #names: #types,)*
            #marker
        }
        #[allow(clippy::new_without_default)]
        impl<#parameters> #name<#struct_arguments>
        where #(#struct_predicates,)*
        {
            #[allow(clippy::too_many_arguments)]
            #vis fn new(#(#names: #types),*) -> Self {
                Self { #(#names,)* #marker_init }
            }
        }
        impl<#parameters #(#family_bounds,)*> #runtime::Operation<#receivers> for #name<#struct_arguments>
        where #(#all_predicates,)*
        {
            type Output = #output;
            type Yield = #yield_tokens;
        }
        impl<'__s, #parameters #(#family_bounds,)* #(#access_parameters,)*>
            #runtime::Start<'__s, (#(#access_parameters,)*)> for #name<#struct_arguments>
        where
            #(#access_parameters: #runtime::ReceiverAccess<Family = #families>,)*
            #(#all_predicates,)*
        {
            type Fut = impl ::core::future::Future<
                Output = <Self as #runtime::Operation<#receivers>>::Output,
            >;
            #[allow(unused_mut, unused_variables)]
            fn start(
                self,
                (#(#locals,)*): (#(#access_parameters,)*),
                __emitter: #runtime::Emitter<'__s, #yield_tokens>,
            ) -> Self::Fut {
                let Self { #(#names,)* .. } = self;
                async move {
                    #(let mut #locals = #locals;)*
                    let mut __emitter = __emitter;
                    #function #turbofish(#(&mut #locals,)* #(#names,)* #emitter_argument).await
                }
            }
        }
    };

    let host = host_trait(HostTrait {
        vis,
        operation: &name,
        function,
        lifetimes: &lifetimes,
        trait_generics: &trait_generics,
        method_generics: &method_generics,
        family_bounds: &family_bounds,
        families: &families,
        struct_arguments: &struct_arguments,
        names: &names,
        types: &types,
        output: &output,
        sync: options.sync,
    });

    if let Some(yields) = &options.yields {
        item.sig
            .inputs
            .push(parse_quote!(#emitter: &mut #runtime::Emitter<'_, #yields>));
    }
    Ok(quote! {
        #item
        #definition
        #host
    })
}

struct HostTrait<'a> {
    vis: &'a syn::Visibility,
    operation: &'a Ident,
    function: &'a Ident,
    lifetimes: &'a [LifetimeParam],
    trait_generics: &'a Generics,
    method_generics: &'a Generics,
    family_bounds: &'a [Tokens],
    families: &'a [&'a Ident],
    struct_arguments: &'a Tokens,
    names: &'a [&'a Ident],
    types: &'a [&'a Type],
    output: &'a Type,
    sync: bool,
}

/// Methods on executors over the operation's families, which bind them and
/// run the operation as a call.
fn host_trait(host: HostTrait<'_>) -> Tokens {
    let runtime = runtime();
    let HostTrait {
        vis,
        operation,
        function,
        lifetimes,
        trait_generics,
        method_generics,
        family_bounds,
        families,
        struct_arguments,
        names,
        types,
        output,
        sync,
    } = host;
    let trait_name = format_ident!("{}HostExt", operation);
    let unpin = format_ident!("{}_unpin", function);
    let trait_parameters: Vec<&GenericParam> = trait_generics.params.iter().collect();
    let trait_names: Vec<Tokens> = trait_parameters
        .iter()
        .map(|parameter| match parameter {
            GenericParam::Type(parameter) => {
                let ident = &parameter.ident;
                quote!(#ident)
            }
            GenericParam::Const(parameter) => {
                let ident = &parameter.ident;
                quote!(#ident)
            }
            GenericParam::Lifetime(parameter) => {
                let lifetime = &parameter.lifetime;
                quote!(#lifetime)
            }
        })
        .collect();
    let trait_predicates: Vec<&WherePredicate> = trait_generics
        .where_clause
        .iter()
        .flat_map(|clause| clause.predicates.iter())
        .collect();
    let method_parameters = &method_generics.params;
    let method_predicates: Vec<&WherePredicate> = method_generics
        .where_clause
        .iter()
        .flat_map(|clause| clause.predicates.iter())
        .collect();
    let receivers = quote!((#(#families,)*));
    let operation_type = quote!(#operation<#struct_arguments>);
    let mut methods = quote! {
        fn #function<'__d, #(#lifetimes,)* #method_parameters>(
            self: ::core::pin::Pin<&'__d mut Self>,
            #(#names: #types),*
        ) -> <Self as #runtime::CallExecutor>::Execution<'__d, #operation_type>
        where #operation_type: '__d, #(#method_predicates,)*
        {
            #runtime::CallExecutor::execute(self, #operation::new(#(#names),*))
        }
        fn #unpin<'__d, #(#lifetimes,)* #method_parameters>(
            &'__d mut self,
            #(#names: #types),*
        ) -> <Self as #runtime::CallExecutor>::Execution<'__d, #operation_type>
        where Self: ::core::marker::Unpin, #operation_type: '__d, #(#method_predicates,)*
        {
            Self::#function(::core::pin::Pin::new(self), #(#names),*)
        }
    };
    if sync {
        let sync_name = format_ident!("sync_{}", function);
        let sync_unpin = format_ident!("sync_{}_unpin", function);
        methods.extend(quote! {
            fn #sync_name<'__d, #(#lifetimes,)* #method_parameters>(
                self: ::core::pin::Pin<&'__d mut Self>,
                #(#names: #types),*
            ) -> #output
            where #operation_type: '__d, #(#method_predicates,)*
            {
                #runtime::__private::run_sync(Self::#function(self, #(#names),*))
            }
            fn #sync_unpin<'__d, #(#lifetimes,)* #method_parameters>(
                &'__d mut self,
                #(#names: #types),*
            ) -> #output
            where Self: ::core::marker::Unpin, #operation_type: '__d, #(#method_predicates,)*
            {
                #runtime::__private::run_sync(Self::#function(::core::pin::Pin::new(self), #(#names),*))
            }
        });
    }
    let doc = format!("Run [`{operation}`] on an executor over its receivers.");
    quote! {
        #[doc = #doc]
        #vis trait #trait_name<#(#trait_parameters,)* #(#families,)*>:
            #runtime::CallExecutor<Receivers = #receivers>
        where #(#family_bounds,)* #(#trait_predicates,)*
        {
            #methods
        }
        impl<#(#trait_parameters,)* #(#families,)* __Executor> #trait_name<#(#trait_names,)* #(#families,)*> for __Executor
        where
            __Executor: #runtime::CallExecutor<Receivers = #receivers> + ?Sized,
            #(#family_bounds,)* #(#trait_predicates,)*
        {}
    }
}
