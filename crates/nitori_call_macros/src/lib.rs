//! Code generation for operations of `nitori_call`.
//!
//! `#[capability]` turns each poll method of a capability trait into an
//! operation struct that is also its future; `#[call]` turns an async function
//! into an operation struct whose future runs the function, rewriting only the
//! `yield` expressions of its body. Both then generate the same conveniences:
//! methods on receiver accesses inside operations, and methods on hosts that
//! bind and run a call.
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::{
    Error, Expr, FnArg, GenericArgument, GenericParam, Generics, Ident, ItemFn, ItemTrait,
    Lifetime, LifetimeParam, Pat, Path, PathArguments, Result, ReturnType, Token, TraitItem,
    TraitItemFn, Type, TypeParam, TypeParamBound, TypeReference, Visibility, WherePredicate,
    parse_macro_input, parse_quote, parse_quote_spanned,
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
    visit_mut::{self, VisitMut},
};

fn runtime() -> Tokens {
    quote!(::nitori_call)
}

/// Generate operations and conveniences for every `poll_*` method of a
/// capability trait.
#[proc_macro_attribute]
pub fn capability(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemTrait);
    let result = Options::parse(attribute).and_then(|options| expand_capability(item, options));
    result.unwrap_or_else(Error::into_compile_error).into()
}

/// Generate an operation and conveniences for an async function whose first
/// parameter is a mutable receiver access.
#[proc_macro_attribute]
pub fn call(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    let result = Options::parse(attribute).and_then(|options| expand_call(item, options));
    result.unwrap_or_else(Error::into_compile_error).into()
}

/// Where generated operation structs live.
enum Module {
    /// The macro's default.
    Default,
    /// `module = self`: the current module.
    Current,
    /// `module = name`: a new module.
    Named(Ident),
}

struct Options {
    module: Module,
    sync: bool,
    /// `yields = Type`: the item type of the function's `yield` expressions.
    yields: Option<Type>,
}
impl Options {
    fn parse(attribute: TokenStream) -> Result<Self> {
        let mut options = Options {
            module: Module::Default,
            sync: false,
            yields: None,
        };
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("module") {
                let value = meta.value()?;
                options.module = if value.peek(Token![self]) {
                    value.parse::<Token![self]>()?;
                    Module::Current
                } else {
                    Module::Named(value.parse()?)
                };
                Ok(())
            } else if meta.path.is_ident("sync") {
                options.sync = true;
                Ok(())
            } else if meta.path.is_ident("yields") {
                options.yields = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta
                    .error("expected `module = name`, `module = self`, `sync` or `yields = Type`"))
            }
        });
        syn::parse::Parser::parse(parser, attribute)?;
        Ok(options)
    }
}

/// One argument after the access, or after the view and context of a poll method.
struct Argument {
    name: Ident,
    ty: Type,
}

/// What the generators need to know about one operation. Argument types,
/// predicates and the output use `Self` to mean the family.
struct Operation {
    vis: Visibility,
    name: Ident,
    struct_name: Ident,
    /// The module holding the struct, if not the current one.
    module: Option<Ident>,
    /// Lifetimes of the struct, with their bounds.
    lifetimes: Vec<LifetimeParam>,
    /// Type and const parameters of the generated traits.
    trait_generics: Generics,
    /// Type and const parameters of the generated methods.
    method_generics: Generics,
    /// Bounds the family must satisfy.
    family_bounds: Punctuated<TypeParamBound, Token![+]>,
    /// For capability operations, the trait as named from the current module.
    capability: Option<Path>,
    arguments: Vec<Argument>,
    output: Type,
    /// The item type of an emitter parameter, for operations that emit.
    yield_type: Option<Type>,
    sync: bool,
}

/// Where generated code is placed: the macro's module, or the struct's module.
#[derive(Clone, Copy)]
enum Place {
    Outside,
    Inside,
}

impl Operation {
    fn names(&self) -> Vec<&Ident> {
        self.arguments
            .iter()
            .map(|argument| &argument.name)
            .collect()
    }
    fn types(&self) -> Vec<&Type> {
        self.arguments.iter().map(|argument| &argument.ty).collect()
    }
    fn capability_at(&self, place: Place) -> Option<Path> {
        let capability = self.capability.as_ref()?;
        Some(match (place, &self.module) {
            (Place::Inside, Some(_)) => parse_quote!(super::#capability),
            _ => capability.clone(),
        })
    }
    fn substitute<T: Substitutable>(&self, mut value: T, family: &Type, place: Place) -> T {
        let capability = self.capability_at(place);
        value.substitute(&mut SelfAsFamily {
            family,
            capability: capability.as_ref(),
        });
        value
    }
    /// The struct as named from outside its module.
    fn struct_path(&self) -> Tokens {
        let name = &self.struct_name;
        match &self.module {
            Some(module) => quote!(#module::#name),
            None => quote!(#name),
        }
    }
    /// Visibility of items inside the struct's module.
    fn inner_vis(&self) -> Visibility {
        match &self.module {
            Some(_) => parse_quote!(pub),
            None => self.vis.clone(),
        }
    }
    /// Type and const parameters of the traits, then of the methods.
    fn value_parameters(&self) -> Vec<&GenericParam> {
        self.trait_generics
            .params
            .iter()
            .chain(self.method_generics.params.iter())
            .collect()
    }
    fn value_arguments(&self) -> Vec<&Ident> {
        self.value_parameters()
            .into_iter()
            .map(|parameter| match parameter {
                GenericParam::Type(parameter) => &parameter.ident,
                GenericParam::Const(parameter) => &parameter.ident,
                GenericParam::Lifetime(_) => unreachable!(),
            })
            .collect()
    }
    fn lifetime_names(&self) -> Vec<&Lifetime> {
        self.lifetimes
            .iter()
            .map(|parameter| &parameter.lifetime)
            .collect()
    }
    /// Generic parameters of the struct and of impls over it, without the access.
    fn parameters(&self) -> Tokens {
        let lifetimes = &self.lifetimes;
        let values = self.value_parameters();
        quote!(#(#lifetimes,)* #(#values,)*)
    }
    /// Generic arguments of the struct, without the access.
    fn arguments(&self) -> Tokens {
        let lifetimes = self.lifetime_names();
        let values = self.value_arguments();
        quote!(#(#lifetimes,)* #(#values,)*)
    }
    fn all_predicates(&self) -> Vec<WherePredicate> {
        [&self.trait_generics, &self.method_generics]
            .into_iter()
            .flat_map(|generics| generics.where_clause.iter())
            .flat_map(|clause| clause.predicates.iter().cloned())
            .collect()
    }
    fn predicates(&self, generics: &Generics, family: &Type, place: Place) -> Vec<WherePredicate> {
        generics
            .where_clause
            .iter()
            .flat_map(|clause| clause.predicates.iter().cloned())
            .map(|predicate| self.substitute(predicate, family, place))
            .collect()
    }
    /// Predicates that do not mention the family, which the struct can carry.
    fn struct_predicates(&self) -> Vec<WherePredicate> {
        self.all_predicates()
            .into_iter()
            .filter(|predicate| !mentions(quote!(#predicate), "Self"))
            .collect()
    }
    /// Predicates of impls over family `family`.
    fn family_predicates(&self, family: &Type, place: Place) -> Vec<WherePredicate> {
        self.all_predicates()
            .into_iter()
            .map(|predicate| self.substitute(predicate, family, place))
            .collect()
    }
    fn family_bound_tokens(&self, place: Place) -> Tokens {
        let runtime = runtime();
        let bounds = match self.capability_at(place) {
            Some(capability) => vec![quote!(#capability)],
            None => self
                .family_bounds
                .iter()
                .map(|bound| quote!(#bound))
                .collect(),
        };
        quote!(#runtime::ReceiverFamily #(+ #bounds)*)
    }
    /// A hidden field keeping every lifetime and type parameter used.
    fn marker(&self) -> Option<(Tokens, Tokens)> {
        let lifetimes = self.lifetime_names();
        let types: Vec<&Ident> = self
            .value_parameters()
            .into_iter()
            .filter_map(|parameter| match parameter {
                GenericParam::Type(parameter) => Some(&parameter.ident),
                _ => None,
            })
            .collect();
        if lifetimes.is_empty() && types.is_empty() {
            return None;
        }
        let vis = self.inner_vis();
        Some((
            quote! {
                #[doc(hidden)]
                #vis __marker: ::core::marker::PhantomData<fn() -> (#(&#lifetimes (),)* #(*const #types,)*)>,
            },
            quote!(__marker: ::core::marker::PhantomData,),
        ))
    }

    /// The struct, its constructor and its `Operation` implementation.
    /// `access` adds a defaulted access parameter for primitive operations.
    fn struct_definition(&self, access: bool) -> Tokens {
        let runtime = runtime();
        let vis = self.inner_vis();
        let name = &self.struct_name;
        let parameters = self.parameters();
        let arguments = self.arguments();
        let names = self.names();
        let types = self.types();
        let predicates = self.struct_predicates();
        let (marker, marker_init) = self.marker().unzip();
        let (access_parameter, access_field, access_init) = if access {
            (
                quote!(__A = ()),
                quote!(#[doc(hidden)] #vis __access: __A,),
                quote!(__access: (),),
            )
        } else {
            Default::default()
        };
        let family: Type = parse_quote!(__F);
        let family_bounds = self.family_bound_tokens(Place::Inside);
        let family_predicates = self.family_predicates(&family, Place::Inside);
        let output = self.substitute(self.output.clone(), &family, Place::Inside);
        let yield_type = match &self.yield_type {
            Some(item) => self.substitute(item.clone(), &family, Place::Inside),
            None => parse_quote!(::core::convert::Infallible),
        };
        let operation = self.name.to_string();
        let doc = format!("The `{operation}` operation.");
        quote! {
            #[doc = #doc]
            #[must_use = "operations do nothing until started and polled"]
            #vis struct #name<#parameters #access_parameter>
            where #(#predicates,)*
            {
                #access_field
                #(#vis #names: #types,)*
                #marker
            }
            #[allow(clippy::new_without_default)]
            impl<#parameters> #name<#arguments>
            where #(#predicates,)*
            {
                #[allow(clippy::too_many_arguments)]
                #vis fn new(#(#names: #types),*) -> Self {
                    Self { #access_init #(#names,)* #marker_init }
                }
            }
            impl<#parameters __F: #family_bounds> #runtime::Operation<__F> for #name<#arguments>
            where #(#family_predicates,)*
            {
                type Output = #output;
                type Yield = #yield_type;
            }
        }
    }

    /// Parameters of a convenience trait over `receiver`, whose predicates
    /// name the receiver's family where the operation says `Self`.
    fn convenience_generics(&self, receiver: &Tokens) -> Generics {
        let family: Type = parse_quote!(<Self as #receiver>::Family);
        let mut generics = self.trait_generics.clone();
        if let Some(clause) = &mut generics.where_clause {
            for predicate in clause.predicates.iter_mut() {
                *predicate = self.substitute(predicate.clone(), &family, Place::Outside);
            }
        }
        generics
    }

    /// Wrap definitions in the struct's module, if it has one.
    fn in_module(&self, definitions: Tokens) -> Tokens {
        match &self.module {
            Some(module) => {
                let vis = &self.vis;
                quote! {
                    #vis mod #module {
                        #[allow(unused_imports)]
                        use super::*;
                        #definitions
                    }
                }
            }
            None => definitions,
        }
    }

    /// Methods that bind a host, run the operation as a call and return its result.
    fn host_methods(&self) -> Tokens {
        let runtime = runtime();
        let family: Type = parse_quote!(<Self as #runtime::CallExecutor>::Family);
        let path = self.struct_path();
        let names = self.names();
        let types = self.types();
        let output = self.substitute(self.output.clone(), &family, Place::Outside);
        let name = &self.name;
        let unpin = format_ident!("{}_unpin", name);
        let lifetimes = &self.lifetimes;
        let parameters = &self.method_generics.params;
        let predicates = self.predicates(&self.method_generics, &family, Place::Outside);
        let arguments = self.arguments();
        let mut methods = quote! {
            fn #name<'__d, #(#lifetimes,)* #parameters>(
                self: ::core::pin::Pin<&'__d mut Self>,
                #(#names: #types),*
            ) -> <Self as #runtime::CallExecutor>::Execution<'__d, #path<#arguments>>
            where #path<#arguments>: '__d, #(#predicates,)*
            {
                #runtime::CallExecutor::execute(self, #path::new(#(#names),*))
            }
            fn #unpin<'__d, #(#lifetimes,)* #parameters>(
                &'__d mut self,
                #(#names: #types),*
            ) -> <Self as #runtime::CallExecutor>::Execution<'__d, #path<#arguments>>
            where Self: ::core::marker::Unpin, #path<#arguments>: '__d, #(#predicates,)*
            {
                Self::#name(::core::pin::Pin::new(self), #(#names),*)
            }
        };
        if self.sync {
            let sync = format_ident!("sync_{}", name);
            let sync_unpin = format_ident!("sync_{}_unpin", name);
            methods.extend(quote! {
                fn #sync<'__d, #(#lifetimes,)* #parameters>(self: ::core::pin::Pin<&'__d mut Self>, #(#names: #types),*) -> #output
                where #path<#arguments>: '__d, #(#predicates,)*
                {
                    #runtime::__private::run_sync(Self::#name(self, #(#names),*))
                }
                fn #sync_unpin<'__d, #(#lifetimes,)* #parameters>(&'__d mut self, #(#names: #types),*) -> #output
                where Self: ::core::marker::Unpin, #path<#arguments>: '__d, #(#predicates,)*
                {
                    #runtime::__private::run_sync(Self::#name(::core::pin::Pin::new(self), #(#names),*))
                }
            });
        }
        methods
    }
}

/// A trait with conveniences, and its blanket implementation.
fn convenience_trait(
    vis: &Visibility,
    name: &Ident,
    generics: &Generics,
    receiver: Tokens,
    family_bounds: Tokens,
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

fn camel_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            let first = characters.next().unwrap().to_ascii_uppercase();
            std::iter::once(first).chain(characters).collect::<String>()
        })
        .collect()
}

fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                snake.push('_');
            }
            snake.push(character.to_ascii_lowercase());
        } else {
            snake.push(character);
        }
    }
    snake
}

fn expand_capability(mut item: ItemTrait, options: Options) -> Result<Tokens> {
    let runtime = runtime();
    if let Some(lifetime) = item.generics.lifetimes().next() {
        return Err(Error::new(
            lifetime.span(),
            "#[capability] does not yet support lifetime parameters of capability traits",
        ));
    }
    let module = match options.module {
        Module::Default => Some(format_ident!("{}", snake_case(&item.ident.to_string()))),
        Module::Current => None,
        Module::Named(module) => Some(module),
    };
    let ident = &item.ident;
    let (_, type_generics, _) = item.generics.split_for_impl();
    let capability: Path = parse_quote!(#ident #type_generics);
    // Generated items declare the trait's parameters without defaults.
    let mut trait_generics = item.generics.clone();
    for parameter in trait_generics.params.iter_mut() {
        match parameter {
            GenericParam::Type(parameter) => {
                parameter.eq_token = None;
                parameter.default = None;
            }
            GenericParam::Const(parameter) => {
                parameter.eq_token = None;
                parameter.default = None;
            }
            GenericParam::Lifetime(_) => {}
        }
    }
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
            .map(String::from)
        else {
            continue;
        };
        let name = format_ident!("{}", name, span = method.sig.ident.span());
        operations.push(primitive(
            &item,
            &capability,
            &trait_generics,
            &module,
            options.sync,
            method,
            name,
        )?);
    }
    if operations.is_empty() {
        return Err(Error::new(
            item.ident.span(),
            "#[capability] found no poll_* methods",
        ));
    }

    let mut definitions = Tokens::new();
    let mut access_methods = Tokens::new();
    let mut host_methods = Tokens::new();
    for (operation, definition, method) in &operations {
        definitions.extend(definition.clone());
        access_methods.extend(method.clone());
        host_methods.extend(operation.host_methods());
    }
    let first = &operations[0].0;
    let mut generated = first.in_module(definitions);
    let vis = &item.vis;
    let bounds = first.family_bound_tokens(Place::Outside);
    let access = quote!(#runtime::ReceiverAccess);
    let host = quote!(#runtime::CallExecutor);
    generated.extend(convenience_trait(
        vis,
        &format_ident!("{}Ext", item.ident),
        &first.convenience_generics(&access),
        access,
        bounds.clone(),
        access_methods,
    ));
    generated.extend(convenience_trait(
        vis,
        &format_ident!("{}HostExt", item.ident),
        &first.convenience_generics(&host),
        host,
        bounds,
        host_methods,
    ));
    item.attrs
        .retain(|attribute| !attribute.path().is_ident("capability"));
    Ok(quote! {
        #item
        #generated
    })
}

/// A primitive operation: its description, struct and future, and the access
/// method that builds the future.
fn primitive(
    item: &ItemTrait,
    capability: &Path,
    trait_generics: &Generics,
    module: &Option<Ident>,
    sync: bool,
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
    let lifetimes = name_lifetimes(&mut arguments, &[], Tokens::new());
    for argument in &arguments {
        let ty = &argument.ty;
        if mentions(quote!(#ty), "Self") {
            return Err(Error::new(
                ty.span(),
                "operation arguments cannot depend on the family",
            ));
        }
    }
    let output = poll_output(&signature.output)?;

    // The view lifetime and `Self: 'view` belong to the poll, not the operation.
    let mut method_generics = Generics {
        params: signature
            .generics
            .params
            .iter()
            .filter(|parameter| !matches!(parameter, GenericParam::Lifetime(_)))
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
        struct_name: format_ident!("{}", camel_case(&name.to_string()), span = name.span()),
        name,
        module: module.clone(),
        lifetimes,
        trait_generics: trait_generics.clone(),
        method_generics,
        family_bounds: Punctuated::new(),
        capability: Some(capability.clone()),
        arguments,
        output,
        yield_type: None,
        sync,
    };

    let struct_name = &operation.struct_name;
    let poll = &signature.ident;
    let parameters = operation.parameters();
    let arguments = operation.arguments();
    let names = operation.names();
    let inside = operation
        .capability_at(Place::Inside)
        .expect("primitive operations have a capability");
    let access_family: Type = parse_quote!(<__A as #runtime::ReceiverAccess>::Family);
    let struct_predicates = operation.struct_predicates();
    let predicates = operation.family_predicates(&access_family, Place::Inside);
    let output = operation.substitute(operation.output.clone(), &access_family, Place::Inside);
    let (_, marker_init) = operation.marker().unzip();
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
    let mut definition = operation.struct_definition(true);
    definition.extend(quote! {
        impl<'__s, #parameters __A> #runtime::Start<'__s, __A> for #struct_name<#arguments>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #inside,
            #(#predicates,)*
        {
            type Fut = #struct_name<#arguments __A>;
            fn start(
                self,
                __io: __A,
                _: #runtime::Emitter<'__s, ::core::convert::Infallible>,
            ) -> Self::Fut {
                #struct_name { __access: __io, #(#names: self.#names,)* #marker_init }
            }
        }
        // Fields are never pinned; the future only reborrows them.
        impl<#parameters __A> ::core::marker::Unpin for #struct_name<#arguments __A>
        where #(#struct_predicates,)*
        {}
        impl<#parameters __A> ::core::future::Future for #struct_name<#arguments __A>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #inside,
            #(#predicates,)*
        {
            type Output = #output;
            fn poll(
                self: ::core::pin::Pin<&mut Self>,
                __cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<Self::Output> {
                let __this = ::core::pin::Pin::get_mut(self);
                #(#bindings)*
                #runtime::ReceiverAccess::poll_access(&mut __this.__access, __cx, move |__access, __cx| {
                    <<__A as #runtime::ReceiverAccess>::Family as #inside>::#poll(
                        __access.into_pin(), __cx, #(#names),*
                    )
                })
            }
        }
    });

    let self_family: Type = parse_quote!(<Self as #runtime::ReceiverAccess>::Family);
    let types = operation.types();
    let lifetimes = &operation.lifetimes;
    let method_parameters = &operation.method_generics.params;
    let method_predicates =
        operation.predicates(&operation.method_generics, &self_family, Place::Outside);
    let path = operation.struct_path();
    let name = &operation.name;
    let method = quote! {
        fn #name<#(#lifetimes,)* '__a, #method_parameters>(
            &'__a mut self,
            #(#names: #types),*
        ) -> #path<#arguments &'__a mut Self>
        where Self: ::core::marker::Sized, #(#method_predicates,)*
        {
            #path { __access: self, #(#names,)* #marker_init }
        }
    };
    Ok((operation, definition, method))
}

fn expand_call(mut item: ItemFn, options: Options) -> Result<Tokens> {
    let runtime = runtime();
    // The emitter parameter added for `yield`, named hygienically.
    let emitter = Ident::new("__emitter", Span::mixed_site());
    match &options.yields {
        Some(_) => RewriteYield { emitter: &emitter }.visit_block_mut(&mut item.block),
        None => {
            let mut find = FindYield(None);
            find.visit_block(&item.block);
            if let Some(span) = find.0 {
                return Err(Error::new(
                    span,
                    "declare the item type of `yield` with `#[call(yields = Type)]`",
                ));
            }
        }
    }
    let signature = &item.sig.clone();
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
    let mut yield_type = options.yields.clone();
    let mut arguments = Vec::new();
    for input in inputs {
        arguments.push(argument(input)?);
    }

    let mut generics = signature.generics.clone();
    let (access_name, family_bounds) = take_access_bounds(&access.ty, &mut generics)?;
    // Explicit arguments for calling the function from the operation's future.
    let turbofish: Vec<Tokens> = signature
        .generics
        .params
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) if Some(&parameter.ident) == access_name.as_ref() => {
                Some(quote!(__A))
            }
            GenericParam::Type(parameter) => {
                let ident = &parameter.ident;
                Some(quote!(#ident))
            }
            GenericParam::Const(parameter) => {
                let ident = &parameter.ident;
                Some(quote!(#ident))
            }
            GenericParam::Lifetime(_) => None,
        })
        .collect();

    let mut output: Type = match &signature.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };
    if let Some(access_name) = &access_name {
        let mut replace = AccessAsSelf {
            access: access_name,
        };
        replace.visit_type_mut(&mut output);
        if let Some(item) = &mut yield_type {
            replace.visit_type_mut(item);
            if mentions(quote!(#item), &access_name.to_string()) {
                return Err(Error::new(
                    access_name.span(),
                    format!("the item type may mention the access only as `{access_name}::Family`"),
                ));
            }
        }
        if let Some(clause) = &mut generics.where_clause {
            for predicate in clause.predicates.iter_mut() {
                replace.visit_where_predicate_mut(predicate);
            }
        }
        let rest = &generics.where_clause;
        for (tokens, what) in [
            (quote!(#output), "the output"),
            (quote!(#rest), "the where clause"),
        ] {
            if mentions(tokens, &access_name.to_string()) {
                return Err(Error::new(
                    access_name.span(),
                    format!("{what} may mention the access only as `{access_name}::Family`"),
                ));
            }
        }
        for argument in &arguments {
            let ty = &argument.ty;
            if mentions(quote!(#ty), &access_name.to_string()) {
                return Err(Error::new(
                    ty.span(),
                    "operation arguments cannot depend on the access",
                ));
            }
        }
    }

    // Lifetimes belong to the struct; argument-position `impl Trait` becomes
    // a parameter of the struct and of the access method.
    let declared: Vec<LifetimeParam> = generics.lifetimes().cloned().collect();
    generics.params = generics
        .params
        .into_iter()
        .filter(|parameter| !matches!(parameter, GenericParam::Lifetime(_)))
        .collect();
    let lifetimes = name_lifetimes(&mut arguments, &declared, quote!(#generics #output));
    let anonymous = name_impl_traits(&mut arguments);
    let (trait_generics, mut method_generics) = split_generics(generics, &family_bounds);
    method_generics
        .params
        .extend(anonymous.into_iter().map(GenericParam::Type));

    let module = match options.module {
        Module::Default | Module::Current => None,
        Module::Named(module) => Some(module),
    };
    let operation = Operation {
        vis: item.vis.clone(),
        name: signature.ident.clone(),
        struct_name: format_ident!(
            "{}",
            camel_case(&signature.ident.to_string()),
            span = signature.ident.span()
        ),
        module,
        lifetimes,
        trait_generics,
        method_generics,
        family_bounds,
        capability: None,
        arguments,
        output,
        yield_type,
        sync: options.sync,
    };

    let function = &operation.name;
    let function_path = match &operation.module {
        Some(_) => quote!(super::#function),
        None => quote!(#function),
    };
    let turbofish = (!turbofish.is_empty()).then(|| quote!(::<#(#turbofish),*>));
    let struct_name = &operation.struct_name;
    let parameters = operation.parameters();
    let arguments = operation.arguments();
    let names = operation.names();
    let access_family: Type = parse_quote!(<__A as #runtime::ReceiverAccess>::Family);
    let bounds = operation.family_bound_tokens(Place::Inside);
    let predicates = operation.family_predicates(&access_family, Place::Inside);
    let mut definition = operation.struct_definition(false);
    let (emitter_type, emitter_argument) = match &operation.yield_type {
        Some(item) => (
            operation.substitute(item.clone(), &access_family, Place::Inside),
            quote!(&mut __emitter),
        ),
        None => (parse_quote!(::core::convert::Infallible), Tokens::new()),
    };
    definition.extend(quote! {
        impl<'__s, #parameters __A> #runtime::Start<'__s, __A> for #struct_name<#arguments>
        where
            __A: #runtime::ReceiverAccess,
            <__A as #runtime::ReceiverAccess>::Family: #bounds,
            #(#predicates,)*
        {
            type Fut = impl ::core::future::Future<
                Output = <Self as #runtime::Operation<<__A as #runtime::ReceiverAccess>::Family>>::Output,
            >;
            #[allow(unused_mut, unused_variables)]
            fn start(
                self,
                __io: __A,
                __emitter: #runtime::Emitter<'__s, #emitter_type>,
            ) -> Self::Fut {
                let Self { #(#names,)* .. } = self;
                async move {
                    let mut __io = __io;
                    let mut __emitter = __emitter;
                    #function_path #turbofish(&mut __io, #(#names,)* #emitter_argument).await
                }
            }
        }
    });
    let definition = operation.in_module(definition);

    let self_family: Type = parse_quote!(<Self as #runtime::ReceiverAccess>::Family);
    let types = operation.types();
    let lifetimes = &operation.lifetimes;
    let method_parameters = &operation.method_generics.params;
    let method_predicates =
        operation.predicates(&operation.method_generics, &self_family, Place::Outside);
    let path = operation.struct_path();
    let access_methods = match &operation.yield_type {
        // An emitting child has a cell of its own for its items.
        Some(_) => quote! {
            fn #function<#(#lifetimes,)* '__a, #method_parameters>(
                &'__a mut self,
                #(#names: #types),*
            ) -> #runtime::Child<&'__a mut Self, #path<#arguments>>
            where Self: ::core::marker::Sized, #(#method_predicates,)*
            {
                #runtime::Child::new(#path::new(#(#names),*), self)
            }
        },
        None => quote! {
            fn #function<#(#lifetimes,)* '__a, #method_parameters>(
                &'__a mut self,
                #(#names: #types),*
            ) -> <#path<#arguments> as #runtime::Start<'static, &'__a mut Self>>::Fut
            where Self: ::core::marker::Sized, #(#method_predicates,)*
            {
                #runtime::Start::start(#path::new(#(#names),*), self, #runtime::__private::detached())
            }
        },
    };
    let camel = &operation.struct_name;
    let bounds = operation.family_bound_tokens(Place::Outside);
    let access = quote!(#runtime::ReceiverAccess);
    let host = quote!(#runtime::CallExecutor);
    let access_trait = convenience_trait(
        &operation.vis,
        &format_ident!("{}Ext", camel),
        &operation.convenience_generics(&access),
        access,
        bounds.clone(),
        access_methods,
    );
    let host_trait = convenience_trait(
        &operation.vis,
        &format_ident!("{}HostExt", camel),
        &operation.convenience_generics(&host),
        host,
        bounds,
        operation.host_methods(),
    );
    // The function itself takes the emitter last.
    if let Some(yields) = &options.yields {
        item.sig
            .inputs
            .push(parse_quote!(#emitter: &mut #runtime::Emitter<'_, #yields>));
    }
    Ok(quote! {
        #item
        #definition
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
        let lifetime_only = matches!(predicate, WherePredicate::Lifetime(_));
        if lifetime_only || mentioned.iter().any(|name| method_names.contains(name)) {
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

fn mentions(tokens: Tokens, name: &str) -> bool {
    let mut names = Vec::new();
    collect_names(tokens, &mut names);
    names.iter().any(|candidate| candidate == name)
}

/// Remove the access type from `generics`; return its name, if it has one,
/// and the family's bounds.
///
/// Accepted forms: `&mut impl ReceiverAccess<Family: Bounds>`, or `&mut A`
/// where `A` is bounded by `ReceiverAccess<Family: Bounds>`.
fn take_access_bounds(
    ty: &Type,
    generics: &mut Generics,
) -> Result<(Option<Ident>, Punctuated<TypeParamBound, Token![+]>)> {
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
        Type::ImplTrait(bounds) => Ok((None, family_bounds(&bounds.bounds, ty)?)),
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
            Ok((Some(ident), family_bounds(&bounds, ty)?))
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

/// Turns each `yield e` of the function body into an emission. `yield` in
/// closures (including coroutines), async blocks, nested items and macro
/// arguments keeps its own meaning.
struct RewriteYield<'a> {
    emitter: &'a Ident,
}
impl VisitMut for RewriteYield<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Closure(_) | Expr::Async(_) | Expr::Macro(_) => {}
            Expr::Yield(expression) => {
                let span = expression.yield_token.span;
                let value: Expr = match expression.expr.take() {
                    Some(value) => {
                        let mut value = *value;
                        self.visit_expr_mut(&mut value);
                        value
                    }
                    None => parse_quote!(()),
                };
                let emitter = self.emitter;
                *expr = parse_quote_spanned!(span=> #emitter.emit(#value).await);
            }
            _ => visit_mut::visit_expr_mut(self, expr),
        }
    }
    fn visit_item_mut(&mut self, _: &mut syn::Item) {}
}

/// Finds a `yield` that `RewriteYield` would rewrite.
struct FindYield(Option<Span>);
impl<'ast> Visit<'ast> for FindYield {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        match expr {
            Expr::Closure(_) | Expr::Async(_) | Expr::Macro(_) => {}
            Expr::Yield(expression) => {
                self.0.get_or_insert(expression.yield_token.span);
            }
            _ => visit::visit_expr(self, expr),
        }
    }
    fn visit_item(&mut self, _: &'ast syn::Item) {}
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

/// Name elided lifetimes in argument types; return them with the declared
/// lifetimes the arguments use.
/// Name elided lifetimes of the arguments and keep the declared lifetimes
/// that the arguments or `elsewhere` (bounds and output) use, directly or
/// through the bounds of another kept lifetime.
fn name_lifetimes(
    arguments: &mut [Argument],
    declared: &[LifetimeParam],
    elsewhere: Tokens,
) -> Vec<LifetimeParam> {
    let mut namer = LifetimeNamer { fresh: Vec::new() };
    for argument in arguments.iter_mut() {
        namer.visit_type_mut(&mut argument.ty);
    }
    let mut used = Vec::new();
    for argument in arguments.iter() {
        let ty = &argument.ty;
        collect_names(quote!(#ty), &mut used);
    }
    collect_names(elsewhere, &mut used);
    let mut kept: Vec<&LifetimeParam> = Vec::new();
    loop {
        let before = kept.len();
        for parameter in declared {
            if !kept.iter().any(|kept| kept.lifetime == parameter.lifetime)
                && used.contains(&parameter.lifetime.to_string())
            {
                let bounds = &parameter.bounds;
                collect_names(quote!(#bounds), &mut used);
                kept.push(parameter);
            }
        }
        if kept.len() == before {
            break;
        }
    }
    declared
        .iter()
        .filter(|parameter| kept.iter().any(|kept| kept.lifetime == parameter.lifetime))
        .cloned()
        .chain(namer.fresh.into_iter().map(LifetimeParam::new))
        .collect()
}

struct LifetimeNamer {
    fresh: Vec<Lifetime>,
}
impl LifetimeNamer {
    fn fresh(&mut self) -> Lifetime {
        let lifetime = Lifetime::new(&format!("'__l{}", self.fresh.len()), Span::call_site());
        self.fresh.push(lifetime.clone());
        lifetime
    }
}
impl VisitMut for LifetimeNamer {
    fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
        match &mut reference.lifetime {
            None => reference.lifetime = Some(self.fresh()),
            Some(lifetime) => self.visit_lifetime_mut(lifetime),
        }
        self.visit_type_mut(&mut reference.elem);
    }
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident == "_" {
            *lifetime = self.fresh();
        }
    }
}

/// Replace argument-position `impl Trait` with fresh type parameters.
fn name_impl_traits(arguments: &mut [Argument]) -> Vec<TypeParam> {
    let mut namer = ImplTraitNamer {
        parameters: Vec::new(),
    };
    for argument in arguments.iter_mut() {
        namer.visit_type_mut(&mut argument.ty);
    }
    namer.parameters
}

struct ImplTraitNamer {
    parameters: Vec<TypeParam>,
}
impl VisitMut for ImplTraitNamer {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::ImplTrait(bounds) = ty {
            let ident = format_ident!("__T{}", self.parameters.len());
            let bounds = &bounds.bounds;
            self.parameters.push(parse_quote!(#ident: #bounds));
            *ty = parse_quote!(#ident);
            return;
        }
        visit_mut::visit_type_mut(self, ty);
    }
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

/// Replaces `A::Family` and `<A as ReceiverAccess>::Family` with `Self`.
struct AccessAsSelf<'a> {
    access: &'a Ident,
}
impl VisitMut for AccessAsSelf<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(path) = ty {
            let segments: Vec<_> = path.path.segments.iter().collect();
            if path.qself.is_none()
                && segments.len() >= 2
                && segments[0].ident == *self.access
                && segments[0].arguments.is_empty()
                && segments[1].ident == "Family"
            {
                let rest: Vec<_> = segments[2..]
                    .iter()
                    .map(|segment| (*segment).clone())
                    .collect();
                *ty = if rest.is_empty() {
                    parse_quote!(Self)
                } else {
                    parse_quote!(Self::#(#rest)::*)
                };
                return;
            }
            if let Some(qself) = &path.qself
                && is_ident(&qself.ty, self.access)
                && qself.position + 1 == segments.len()
                && segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Family")
            {
                *ty = parse_quote!(Self);
                return;
            }
        }
        visit_mut::visit_type_mut(self, ty);
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
