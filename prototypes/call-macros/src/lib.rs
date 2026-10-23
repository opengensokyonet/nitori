//! Signature-only code generation for operations of `nitori-call-prototype`.
//!
//! Neither macro rewrites a function body. `#[capability]` turns each poll
//! method of a capability trait into an operation; `#[call]` treats an async
//! function as an operation. Both then generate the same conveniences: methods
//! on receiver accesses inside operations, and methods on hosts that bind and
//! run a call.
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::{
    Error, FnArg, GenericArgument, GenericParam, Generics, Ident, ItemFn, ItemTrait, Pat, Path,
    PathArguments, Result, ReturnType, Token, TraitItem, TraitItemFn, Type, TypeParamBound,
    Visibility, WherePredicate, parse_macro_input, parse_quote,
    punctuated::Punctuated,
    spanned::Spanned,
    visit_mut::{self, VisitMut},
};

fn runtime() -> Tokens {
    quote!(::nitori_call_prototype)
}

/// Generate operations and conveniences for every `poll_*` method of a
/// capability trait.
#[proc_macro_attribute]
pub fn capability(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemTrait);
    let result = if attribute.is_empty() {
        expand_capability(item)
    } else {
        Err(Error::new(
            Tokens::from(attribute).span(),
            "#[capability] takes no arguments",
        ))
    };
    result.unwrap_or_else(Error::into_compile_error).into()
}

/// Generate conveniences for an async function whose first parameter is a
/// mutable receiver access.
#[proc_macro_attribute]
pub fn call(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    let result = if attribute.is_empty() {
        expand_call(item)
    } else {
        Err(Error::new(
            Tokens::from(attribute).span(),
            "#[call] takes no arguments",
        ))
    };
    result.unwrap_or_else(Error::into_compile_error).into()
}

/// One argument after the access, or after the view and context of a poll method.
struct Argument {
    name: Ident,
    ty: Type,
}

/// What the convenience generator needs to know about one operation.
struct Operation {
    vis: Visibility,
    name: Ident,
    /// Parameters of the generated traits, excluding the access and the family.
    trait_generics: Generics,
    /// Parameters of the generated methods.
    method_generics: Generics,
    /// Bounds the family must satisfy.
    family_bounds: Punctuated<TypeParamBound, Token![+]>,
    /// For capability operations, the trait that `Self::Name` types refer to.
    capability: Option<Path>,
    /// Types may mention `Self`, meaning the family, for capability operations.
    arguments: Vec<Argument>,
    output: Type,
}

impl Operation {
    fn root_name(&self) -> Ident {
        format_ident!("__nitori_root_{}", self.name)
    }
    fn names(&self) -> Vec<&Ident> {
        self.arguments
            .iter()
            .map(|argument| &argument.name)
            .collect()
    }
    /// Argument types with `Self` replaced by `family`.
    fn types_for(&self, family: &Type) -> Vec<Type> {
        self.arguments
            .iter()
            .map(|argument| self.substitute(argument.ty.clone(), family))
            .collect()
    }
    fn substitute<T: Substitutable>(&self, mut value: T, family: &Type) -> T {
        value.substitute(&mut SelfAsFamily {
            family,
            capability: self.capability.as_ref(),
        });
        value
    }
    /// Type and const parameters of the traits, then of the methods, in order.
    fn value_parameters(&self) -> Vec<&GenericParam> {
        self.trait_generics
            .params
            .iter()
            .chain(self.method_generics.params.iter())
            .filter(|parameter| !matches!(parameter, GenericParam::Lifetime(_)))
            .collect()
    }
    fn type_arguments(&self) -> Vec<&Ident> {
        self.value_parameters()
            .into_iter()
            .map(|parameter| match parameter {
                GenericParam::Type(parameter) => &parameter.ident,
                GenericParam::Const(parameter) => &parameter.ident,
                GenericParam::Lifetime(_) => unreachable!(),
            })
            .collect()
    }
    fn where_predicates(&self, generics: &Generics, family: &Type) -> Vec<WherePredicate> {
        generics
            .where_clause
            .iter()
            .flat_map(|clause| clause.predicates.iter().cloned())
            .map(|predicate| self.substitute(predicate, family))
            .collect()
    }

    /// The private root function that a host binding starts as a call.
    fn root_function(&self, body: Tokens) -> Tokens {
        let runtime = runtime();
        let root = self.root_name();
        let family: Type = parse_quote!(__F);
        let names = self.names();
        let types = self.types_for(&family);
        let output = self.substitute(self.output.clone(), &family);
        let bounds = &self.family_bounds;
        let lifetimes = self
            .trait_generics
            .lifetimes()
            .chain(self.method_generics.lifetimes());
        let types_parameters = self.value_parameters();
        let mut predicates = self.where_predicates(&self.trait_generics, &family);
        predicates.extend(self.where_predicates(&self.method_generics, &family));
        quote! {
            #[doc(hidden)]
            #[allow(clippy::needless_lifetimes)]
            async fn #root<#(#lifetimes,)* __F: #runtime::ReceiverFamily + #bounds, #(#types_parameters),*>(
                mut __io: #runtime::ReceiverSlot<'_, __F>,
                _: #runtime::Emitter<'_, ::core::convert::Infallible>,
                (#(#names,)*): (#(#types,)*),
            ) -> #output
            where #(#predicates,)*
            {
                let __io = &mut __io;
                #body
            }
        }
    }

    /// Methods that bind a host, run the operation as a call and return its result.
    fn host_methods(&self) -> Tokens {
        let runtime = runtime();
        let family: Type = parse_quote!(<Self as #runtime::Receiver>::Family);
        let root = self.root_name();
        let names = self.names();
        let types = self.types_for(&family);
        let output = self.substitute(self.output.clone(), &family);
        let type_arguments = self.type_arguments();
        let name = &self.name;
        let unpin = format_ident!("{}_unpin", name);
        let sync = format_ident!("sync_{}", name);
        let sync_unpin = format_ident!("sync_{}_unpin", name);
        let parameters = &self.method_generics.params;
        let predicates = self.where_predicates(&self.method_generics, &family);
        quote! {
            fn #name<#parameters>(
                self: ::core::pin::Pin<&mut Self>,
                #(#names: #types),*
            ) -> impl ::core::future::Future<Output = #output>
            where #(#predicates,)*
            {
                #runtime::__private::bind(self, (#root::<#family, #(#type_arguments),*>, (#(#names,)*)))
            }
            fn #unpin<#parameters>(&mut self, #(#names: #types),*) -> impl ::core::future::Future<Output = #output>
            where Self: ::core::marker::Unpin, #(#predicates,)*
            {
                Self::#name(::core::pin::Pin::new(self), #(#names),*)
            }
            fn #sync<#parameters>(self: ::core::pin::Pin<&mut Self>, #(#names: #types),*) -> #output
            where #(#predicates,)*
            {
                #runtime::__private::run_sync(Self::#name(self, #(#names),*))
            }
            fn #sync_unpin<#parameters>(&mut self, #(#names: #types),*) -> #output
            where Self: ::core::marker::Unpin, #(#predicates,)*
            {
                #runtime::__private::run_sync(Self::#name(::core::pin::Pin::new(self), #(#names),*))
            }
        }
    }
}

/// A trait with conveniences, and its blanket implementation.
fn convenience_trait(
    vis: &Visibility,
    name: &Ident,
    generics: &Generics,
    receiver: Tokens,
    family_bounds: &Punctuated<TypeParamBound, Token![+]>,
    methods: Tokens,
) -> Tokens {
    let (_, type_generics, _) = generics.split_for_impl();
    let parameters: Vec<&GenericParam> = generics.params.iter().collect();
    let predicates: Vec<&WherePredicate> = generics
        .where_clause
        .iter()
        .flat_map(|clause| clause.predicates.iter())
        .collect();
    quote! {
        #vis trait #name<#(#parameters),*>: #receiver
        where <Self as #receiver>::Family: #family_bounds, #(#predicates,)*
        {
            #methods
        }
        impl<#(#parameters,)* __Receiver: #receiver + ?Sized> #name #type_generics for __Receiver
        where <__Receiver as #receiver>::Family: #family_bounds, #(#predicates,)*
        {}
    }
}

fn camel_case(name: &Ident) -> String {
    name.to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            let first = characters.next().unwrap().to_ascii_uppercase();
            std::iter::once(first).chain(characters).collect::<String>()
        })
        .collect()
}

fn expand_capability(mut item: ItemTrait) -> Result<Tokens> {
    let runtime = runtime();
    if !item.generics.params.is_empty() {
        return Err(Error::new(
            item.generics.span(),
            "#[capability] does not yet support generic capability traits",
        ));
    }
    let capability: Path = item.ident.clone().into();
    let mut family_bounds = Punctuated::new();
    family_bounds.push(TypeParamBound::Trait(parse_quote!(#capability)));
    let mut operations = Vec::new();
    for member in &item.items {
        let TraitItem::Fn(method) = member else {
            continue;
        };
        let Some(name) = method
            .sig
            .ident
            .to_string()
            .strip_prefix("poll_")
            .map(|name| format_ident!("{}", name))
        else {
            continue;
        };
        operations.push(primitive(&item, &capability, &family_bounds, method, name)?);
    }
    if operations.is_empty() {
        return Err(Error::new(
            item.ident.span(),
            "#[capability] found no poll_* methods",
        ));
    }

    let mut generated = Tokens::new();
    let mut access_methods = Tokens::new();
    let mut host_methods = Tokens::new();
    for (operation, future, method) in &operations {
        generated.extend(future.clone());
        let body = {
            let name = &operation.name;
            let names = operation.names();
            let ext = format_ident!("{}Ext", item.ident);
            quote!(#ext::#name(__io, #(#names),*).await)
        };
        generated.extend(operation.root_function(body));
        access_methods.extend(method.clone());
        host_methods.extend(operation.host_methods());
    }
    let vis = &item.vis;
    let empty = Generics::default();
    generated.extend(convenience_trait(
        vis,
        &format_ident!("{}Ext", item.ident),
        &empty,
        quote!(#runtime::ReceiverAccess),
        &family_bounds,
        access_methods,
    ));
    generated.extend(convenience_trait(
        vis,
        &format_ident!("{}HostExt", item.ident),
        &empty,
        quote!(#runtime::Receiver),
        &family_bounds,
        host_methods,
    ));
    item.attrs
        .retain(|attribute| !attribute.path().is_ident("capability"));
    Ok(quote! {
        #item
        #generated
    })
}

/// A primitive operation: its future type, and the access method that builds it.
fn primitive(
    item: &ItemTrait,
    capability: &Path,
    family_bounds: &Punctuated<TypeParamBound, Token![+]>,
    method: &TraitItemFn,
    name: Ident,
) -> Result<(Operation, Tokens, Tokens)> {
    let runtime = runtime();
    let signature = &method.sig;
    let mut inputs = signature.inputs.iter();
    let (Some(FnArg::Typed(_)), Some(FnArg::Typed(_))) = (inputs.next(), inputs.next()) else {
        return Err(Error::new(
            signature.span(),
            "a poll method takes the view and the context first",
        ));
    };
    let mut arguments = Vec::new();
    for input in inputs {
        arguments.push(argument(input)?);
    }
    let output = poll_output(&signature.output)?;

    // The view lifetime and `Self: 'view` belong to the poll, not the operation.
    let mut method_generics = Generics {
        params: signature
            .generics
            .params
            .iter()
            .filter(|parameter| matches!(parameter, GenericParam::Type(_)))
            .cloned()
            .collect(),
        ..Generics::default()
    };
    if let Some(clause) = &signature.generics.where_clause {
        let predicates: Punctuated<WherePredicate, Token![,]> = clause
            .predicates
            .iter()
            .filter(|predicate| !bounds_self(predicate))
            .cloned()
            .collect();
        if !predicates.is_empty() {
            method_generics.where_clause = Some(parse_quote!(where #predicates));
        }
    }
    let operation = Operation {
        vis: item.vis.clone(),
        name,
        trait_generics: Generics::default(),
        method_generics,
        family_bounds: family_bounds.clone(),
        capability: Some(capability.clone()),
        arguments,
        output,
    };

    let future = format_ident!("{}Future", camel_case(&operation.name));
    let poll = &signature.ident;
    let vis = &operation.vis;
    let access_family: Type = parse_quote!(<__A as #runtime::ReceiverAccess>::Family);
    let names = operation.names();
    let field_types: Vec<Type> = operation
        .types_for(&access_family)
        .into_iter()
        .map(with_lifetime)
        .collect();
    let parameters = &operation.method_generics.params;
    let type_arguments = operation.type_arguments();
    let predicates = operation.where_predicates(&operation.method_generics, &access_family);
    let output = operation.substitute(operation.output.clone(), &access_family);
    let bindings = operation.arguments.iter().map(|argument| {
        let name = &argument.name;
        match &argument.ty {
            Type::Reference(reference) if reference.mutability.is_some() => {
                quote!(let #name = &mut *__this.#name;)
            }
            Type::Reference(_) => quote!(let #name = __this.#name;),
            _ => quote!(let #name = ::core::clone::Clone::clone(&__this.#name);),
        }
    });
    let future_tokens = quote! {
        #[must_use = "operations do nothing unless polled"]
        #vis struct #future<'__a, __A, #parameters>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #capability,
            #(#predicates,)*
        {
            io: &'__a mut __A,
            #(#names: #field_types,)*
        }
        // Fields are never pinned; the future only reborrows them.
        impl<'__a, __A, #parameters> ::core::marker::Unpin for #future<'__a, __A, #(#type_arguments),*>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #capability,
            #(#predicates,)*
        {}
        impl<'__a, __A, #parameters> ::core::future::Future for #future<'__a, __A, #(#type_arguments),*>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #capability,
            #(#predicates,)*
        {
            type Output = #output;
            fn poll(
                self: ::core::pin::Pin<&mut Self>,
                __cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<#output> {
                let __this = ::core::pin::Pin::get_mut(self);
                #(#bindings)*
                #runtime::ReceiverAccess::poll_access(&mut *__this.io, __cx, move |__access, __cx| {
                    <<__A as #runtime::ReceiverAccess>::Family as #capability>::#poll(
                        __access.into_pin(), __cx, #(#names),*
                    )
                })
            }
        }
    };

    let self_family: Type = parse_quote!(<Self as #runtime::ReceiverAccess>::Family);
    let method_types: Vec<Type> = operation
        .types_for(&self_family)
        .into_iter()
        .map(with_lifetime)
        .collect();
    let method_predicates = operation.where_predicates(&operation.method_generics, &self_family);
    let name = &operation.name;
    let method = quote! {
        fn #name<'__a, #parameters>(&'__a mut self, #(#names: #method_types),*) -> #future<'__a, Self, #(#type_arguments),*>
        where Self: ::core::marker::Sized, #(#method_predicates,)*
        {
            #future { io: self, #(#names),* }
        }
    };
    Ok((operation, future_tokens, method))
}

fn expand_call(item: ItemFn) -> Result<Tokens> {
    let runtime = runtime();
    let signature = &item.sig;
    if signature.asyncness.is_none() {
        return Err(Error::new(
            signature.span(),
            "#[call] applies to async functions",
        ));
    }
    let mut inputs = signature.inputs.iter();
    let Some(FnArg::Typed(access)) = inputs.next() else {
        return Err(Error::new(
            signature.span(),
            "the first parameter must be a mutable receiver access",
        ));
    };
    let mut arguments = Vec::new();
    for input in inputs {
        arguments.push(argument(input)?);
    }
    let output = match &signature.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };

    let mut generics = signature.generics.clone();
    let family_bounds = take_access_bounds(&access.ty, &mut generics)?;
    let (trait_generics, method_generics) = split_generics(generics, &family_bounds);
    let operation = Operation {
        vis: item.vis.clone(),
        name: signature.ident.clone(),
        trait_generics,
        method_generics,
        family_bounds,
        capability: None,
        arguments,
        output,
    };
    let name = &operation.name;
    let names = operation.names();
    let types = &operation
        .arguments
        .iter()
        .map(|argument| argument.ty.clone())
        .collect::<Vec<_>>();
    let output = &operation.output;
    let root = operation.root_function(quote!(#name(__io, #(#names),*).await));
    let camel = camel_case(name);
    let parameters = &operation.method_generics.params;
    let predicates = operation
        .method_generics
        .where_clause
        .iter()
        .flat_map(|clause| clause.predicates.iter());
    let access_methods = quote! {
        fn #name<#parameters>(&mut self, #(#names: #types),*) -> impl ::core::future::Future<Output = #output>
        where Self: ::core::marker::Sized, #(#predicates,)*
        {
            #name(self, #(#names),*)
        }
    };
    let access_trait = convenience_trait(
        &operation.vis,
        &format_ident!("{}Ext", camel),
        &operation.trait_generics,
        quote!(#runtime::ReceiverAccess),
        &operation.family_bounds,
        access_methods,
    );
    let host_trait = convenience_trait(
        &operation.vis,
        &format_ident!("{}HostExt", camel),
        &operation.trait_generics,
        quote!(#runtime::Receiver),
        &operation.family_bounds,
        operation.host_methods(),
    );
    Ok(quote! {
        #item
        #root
        #access_trait
        #host_trait
    })
}

/// Parameters named by the family bounds belong to the generated traits, the
/// rest to their methods, so callers can still name them with turbofish.
fn split_generics(
    generics: Generics,
    family_bounds: &Punctuated<TypeParamBound, Token![+]>,
) -> (Generics, Generics) {
    let mut named = Vec::new();
    collect_names(quote!(#family_bounds), &mut named);
    let name_of = |parameter: &GenericParam| match parameter {
        GenericParam::Type(parameter) => parameter.ident.to_string(),
        GenericParam::Const(parameter) => parameter.ident.to_string(),
        GenericParam::Lifetime(parameter) => parameter.lifetime.to_string(),
    };
    let (trait_parameters, method_parameters): (Vec<_>, Vec<_>) = generics
        .params
        .into_iter()
        .partition(|parameter| named.contains(&name_of(parameter)));
    let method_names: Vec<String> = method_parameters.iter().map(name_of).collect();
    let (mut trait_predicates, mut method_predicates) = (Punctuated::new(), Punctuated::new());
    for predicate in generics
        .where_clause
        .into_iter()
        .flat_map(|clause| clause.predicates)
    {
        let mut mentioned = Vec::new();
        collect_names(quote!(#predicate), &mut mentioned);
        if mentioned.iter().any(|name| method_names.contains(name)) {
            method_predicates.push(predicate);
        } else {
            trait_predicates.push(predicate);
        }
    }
    let build = |parameters: Vec<GenericParam>,
                 predicates: Punctuated<WherePredicate, Token![,]>| Generics {
        params: parameters.into_iter().collect(),
        where_clause: (!predicates.is_empty()).then(|| parse_quote!(where #predicates)),
        ..Generics::default()
    };
    (
        build(trait_parameters, trait_predicates),
        build(method_parameters, method_predicates),
    )
}

fn collect_names(tokens: Tokens, names: &mut Vec<String>) {
    let mut previous_apostrophe = false;
    for tree in tokens {
        match tree {
            proc_macro2::TokenTree::Group(group) => collect_names(group.stream(), names),
            proc_macro2::TokenTree::Ident(ident) => {
                names.push(if previous_apostrophe {
                    format!("'{ident}")
                } else {
                    ident.to_string()
                });
            }
            proc_macro2::TokenTree::Punct(punct) => {
                previous_apostrophe = punct.as_char() == '\'';
                continue;
            }
            proc_macro2::TokenTree::Literal(_) => {}
        }
        previous_apostrophe = false;
    }
}

/// Remove the access type from `generics` and return the family's bounds.
///
/// Accepted forms: `&mut impl ReceiverAccess<Family: Bounds>`, or `&mut A`
/// where `A` is bounded by `ReceiverAccess<Family: Bounds>`.
fn take_access_bounds(
    ty: &Type,
    generics: &mut Generics,
) -> Result<Punctuated<TypeParamBound, Token![+]>> {
    let Type::Reference(reference) = ty else {
        return Err(Error::new(
            ty.span(),
            "the access must be a mutable reference",
        ));
    };
    if reference.mutability.is_none() {
        return Err(Error::new(
            ty.span(),
            "the access must be a mutable reference",
        ));
    }
    match &*reference.elem {
        Type::ImplTrait(bounds) => family_bounds(&bounds.bounds, ty),
        Type::Path(path) if path.qself.is_none() && path.path.get_ident().is_some() => {
            let ident = path.path.get_ident().unwrap().clone();
            let position = generics
                .params
                .iter()
                .position(
                    |parameter| matches!(parameter, GenericParam::Type(t) if t.ident == ident),
                )
                .ok_or_else(|| {
                    Error::new(
                        ty.span(),
                        "the access type must be a parameter of this function",
                    )
                })?;
            let parameters: Vec<GenericParam> = generics.params.iter().cloned().collect();
            let GenericParam::Type(parameter) = &parameters[position] else {
                unreachable!()
            };
            let mut bounds: Punctuated<TypeParamBound, Token![+]> = parameter.bounds.clone();
            generics.params = parameters
                .into_iter()
                .enumerate()
                .filter(|(index, _)| *index != position)
                .map(|(_, parameter)| parameter)
                .collect();
            if let Some(clause) = &mut generics.where_clause {
                let mut kept = Punctuated::new();
                for predicate in clause.predicates.iter() {
                    match predicate {
                        WherePredicate::Type(typed) if is_ident(&typed.bounded_ty, &ident) => {
                            bounds.extend(typed.bounds.iter().cloned());
                        }
                        other => kept.push(other.clone()),
                    }
                }
                clause.predicates = kept;
                if clause.predicates.is_empty() {
                    generics.where_clause = None;
                }
            }
            family_bounds(&bounds, ty)
        }
        _ => Err(Error::new(
            ty.span(),
            "the access must be `&mut impl ReceiverAccess<..>` or `&mut A`",
        )),
    }
}

fn family_bounds(
    bounds: &Punctuated<TypeParamBound, Token![+]>,
    span: &Type,
) -> Result<Punctuated<TypeParamBound, Token![+]>> {
    for bound in bounds {
        let TypeParamBound::Trait(trait_bound) = bound else {
            continue;
        };
        let Some(last) = trait_bound.path.segments.last() else {
            continue;
        };
        if last.ident != "ReceiverAccess" {
            continue;
        }
        let PathArguments::AngleBracketed(arguments) = &last.arguments else {
            return Ok(Punctuated::new());
        };
        for argument in &arguments.args {
            if let GenericArgument::Constraint(constraint) = argument
                && constraint.ident == "Family"
            {
                return Ok(constraint.bounds.clone());
            }
        }
        return Ok(Punctuated::new());
    }
    Err(Error::new(
        span.span(),
        "the access must be bounded by `ReceiverAccess<Family: ..>`",
    ))
}

fn is_ident(ty: &Type, ident: &Ident) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident(ident))
}

fn argument(input: &FnArg) -> Result<Argument> {
    let FnArg::Typed(typed) = input else {
        return Err(Error::new(
            input.span(),
            "operations take no self parameter",
        ));
    };
    let Pat::Ident(pattern) = &*typed.pat else {
        return Err(Error::new(
            typed.pat.span(),
            "operation arguments must be named",
        ));
    };
    Ok(Argument {
        name: pattern.ident.clone(),
        ty: (*typed.ty).clone(),
    })
}

fn poll_output(output: &ReturnType) -> Result<Type> {
    if let ReturnType::Type(_, ty) = output
        && let Type::Path(path) = &**ty
        && let Some(last) = path.path.segments.last()
        && last.ident == "Poll"
        && let PathArguments::AngleBracketed(arguments) = &last.arguments
        && let Some(GenericArgument::Type(inner)) = arguments.args.first()
    {
        return Ok(inner.clone());
    }
    Err(Error::new(output.span(), "a poll method returns `Poll<T>`"))
}

fn bounds_self(predicate: &WherePredicate) -> bool {
    matches!(predicate, WherePredicate::Type(typed) if matches!(&typed.bounded_ty, Type::Path(path) if path.path.is_ident("Self")))
}

/// Give a reference argument the future's borrow lifetime.
fn with_lifetime(ty: Type) -> Type {
    match ty {
        Type::Reference(mut reference) => {
            reference.lifetime = Some(parse_quote!('__a));
            Type::Reference(reference)
        }
        other => other,
    }
}

/// Replaces `Self` and `Self::Name` with the family of the current context.
struct SelfAsFamily<'a> {
    family: &'a Type,
    capability: Option<&'a Path>,
}
impl VisitMut for SelfAsFamily<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(path) = ty
            && path.qself.is_none()
            && path
                .path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == "Self")
        {
            let family = self.family;
            let rest: Vec<_> = path.path.segments.iter().skip(1).cloned().collect();
            *ty = match (rest.is_empty(), self.capability) {
                (true, _) => family.clone(),
                (false, Some(capability)) => parse_quote!(<#family as #capability>::#(#rest)::*),
                (false, None) => parse_quote!(#family::#(#rest)::*),
            };
            return;
        }
        visit_mut::visit_type_mut(self, ty);
    }
}

trait Substitutable {
    fn substitute(&mut self, visitor: &mut SelfAsFamily<'_>);
}
impl Substitutable for Type {
    fn substitute(&mut self, visitor: &mut SelfAsFamily<'_>) {
        visitor.visit_type_mut(self);
    }
}
impl Substitutable for WherePredicate {
    fn substitute(&mut self, visitor: &mut SelfAsFamily<'_>) {
        visitor.visit_where_predicate_mut(self);
    }
}
