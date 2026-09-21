use super::*;

// These names have definition-site local-variable hygiene. Source code cannot
// capture or shadow the environment, even by spelling the same identifier.
fn private(name: &str) -> Ident {
    Ident::new(name, Span::mixed_site())
}
fn fresh_parameter(generics: &Generics, base: &str) -> Ident {
    let mut name = base.to_owned();
    while generics.params.iter().any(|p| match p {
        GenericParam::Type(p) => p.ident == name,
        GenericParam::Const(p) => p.ident == name,
        GenericParam::Lifetime(_) => false,
    }) {
        name.push('_');
    }
    private(&name)
}
fn fresh_lifetime(generics: &Generics, base: &str) -> Lifetime {
    let mut name = base.to_owned();
    while generics.lifetimes().any(|p| p.lifetime.ident == name) {
        name.push('_');
    }
    Lifetime::new(&format!("'{name}"), Span::mixed_site())
}
// These spellings are intrinsic to the macro, not arbitrary user macro calls.
fn pin_intrinsic(path: &Path) -> bool {
    let names: Vec<_> = path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect();
    matches!(names.as_slice(), [name] if name == "pin")
        || matches!(names.as_slice(), [root, module, name]
            if (root == "std" || root == "core") && module == "pin" && name == "pin")
}
struct Validate {
    error: Option<Error>,
}
impl VisitMut for Validate {
    fn visit_attribute_mut(&mut self, attribute: &mut Attribute) {
        self.error = Some(Error::new_spanned(
            attribute,
            "attributes are outside the call macro safety boundary",
        ));
    }
    fn visit_macro_mut(&mut self, invocation: &mut Macro) {
        if pin_intrinsic(&invocation.path) {
            match syn::parse2::<Expr>(invocation.tokens.clone()) {
                Ok(mut expression) => self.visit_expr_mut(&mut expression),
                Err(error) => self.error = Some(error),
            }
            return;
        }
        self.error = Some(Error::new_spanned(
            invocation,
            "opaque macros are outside the call macro safety boundary",
        ));
    }
}
struct Rewrite {
    environment: Ident,
    has_yield: bool,
    error: Option<Error>,
}
impl Rewrite {
    fn awaited(&self, input: Tokens) -> Expr {
        let environment = &self.environment;
        let state = private("__stack_child");
        let poll = quote!(#environment.poll_await(#state.as_mut()));
        let input_name = private("__await_input");
        parse_quote!({
            let #input_name = #input;
            let mut #state=::core::pin::pin!(#environment.prepare_await(#input_name));
            loop {
                match unsafe {#poll} {
                    ::core::task::Poll::Ready(value)=>break value,
                    ::core::task::Poll::Pending=>{
                        #environment.end();
                        #environment=yield ::nitori_call::__private::Suspend::Pending;
                    }
                }
            }
        })
    }
}
impl VisitMut for Rewrite {
    fn visit_expr_mut(&mut self, expression: &mut Expr) {
        if let Expr::Macro(invocation) = expression
            && pin_intrinsic(&invocation.mac.path)
        {
            match syn::parse2::<Expr>(invocation.mac.tokens.clone()) {
                Ok(mut inner) => {
                    self.visit_expr_mut(&mut inner);
                    *expression = parse_quote!(::core::pin::pin!(#inner));
                }
                Err(error) => self.error = Some(error),
            }
            return;
        }
        if let Expr::Await(awaited) = expression {
            let mut value = (*awaited.base).clone();
            self.visit_expr_mut(&mut value);
            *expression = self.awaited(quote!(#value));
            return;
        }
        if let Expr::Yield(emitted) = expression {
            self.has_yield = true;
            let mut value = emitted
                .expr
                .as_deref()
                .cloned()
                .unwrap_or_else(|| parse_quote!(()));
            self.visit_expr_mut(&mut value);
            let environment = &self.environment;
            let item = private("__stack_item");
            // Evaluate the item while access is still valid, then end the visit.
            *expression = parse_quote!({
                let #item=#value;
                #environment.end();
                #environment=yield ::nitori_call::__private::Suspend::Emit(#item);
            });
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }
    // These have their own execution/suspension scope. Validation still examines
    // their tokens for opaque macros/attributes, but lowering leaves them intact.
    fn visit_expr_async_mut(&mut self, _: &mut ExprAsync) {}
    fn visit_expr_closure_mut(&mut self, _: &mut ExprClosure) {}
    fn visit_item_mut(&mut self, _: &mut Item) {}
}
// The first parameter selects the stable family; it becomes a real receiver.
fn host_type(annotation: &Type) -> Result<Box<Type>> {
    if let Type::Path(path) = annotation
        && path.qself.is_none()
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Receiver"
        && let PathArguments::AngleBracketed(arguments) = &segment.arguments
        && arguments.args.len() == 1
        && let Some(GenericArgument::Type(family)) = arguments.args.first()
    {
        return Ok(Box::new(family.clone()));
    }
    Err(Error::new_spanned(
        annotation,
        "call receiver must be Receiver<Family>",
    ))
}

fn coroutine(mut closure: ExprClosure) -> Result<(Tokens, Box<Type>, bool)> {
    let mut validate = Validate { error: None };
    validate.visit_expr_closure_mut(&mut closure);
    if let Some(error) = validate.error {
        return Err(error);
    }
    if closure.inputs.len() != 1 {
        return Err(Error::new_spanned(
            &closure.inputs,
            "expected |io: Receiver<F>| { ... }",
        ));
    }
    if closure.asyncness.is_some() || closure.constness.is_some() || closure.lifetimes.is_some() {
        return Err(Error::new_spanned(
            &closure,
            "use a plain closure; await is supported inside its body",
        ));
    }
    if let Some(Pat::Ident(binding)) = closure.inputs.first() {
        let binding = binding.clone();
        closure.inputs[0] = Pat::Type(parse_quote!(#binding: ::nitori_call::Receiver<_>));
    }
    let Some(Pat::Type(parameter)) = closure.inputs.first() else {
        return Err(Error::new_spanned(
            &closure.inputs,
            "expected a receiver host identifier",
        ));
    };
    let Pat::Ident(binding) = &*parameter.pat else {
        return Err(Error::new_spanned(parameter, "host identifier required"));
    };
    if binding.by_ref.is_some() || binding.subpat.is_some() {
        return Err(Error::new_spanned(
            binding,
            "receiver parameter must be a plain identifier",
        ));
    }
    let receiver = parameter.pat.clone();
    let host = host_type(&parameter.ty)?;
    let environment = private("__stack_environment");
    let mut rewrite = Rewrite {
        environment: environment.clone(),
        has_yield: false,
        error: None,
    };
    rewrite.visit_expr_mut(&mut closure.body);
    if let Some(error) = rewrite.error {
        return Err(error);
    }
    let body = match *closure.body {
        Expr::Block(block) => {
            let statements = block.block.stmts;
            quote!(#(#statements)*)
        }
        expression => quote!({#expression}),
    };
    let annotation = &parameter.ty;
    let capture = closure.capture;
    let output = closure.output;
    Ok((
        quote!(::core::convert::identity(#[coroutine] static #capture |mut #environment: ::nitori_call::__private::ResumeEnv<#host>| #output { #[allow(unused_variables)] let #receiver: #annotation = #environment.receiver(); #body })),
        host,
        rewrite.has_yield,
    ))
}

pub(super) fn expand(closure: ExprClosure) -> Result<Tokens> {
    let (coroutine, host, has_yield) = coroutine(closure)?;
    let state = private("__stack_state");
    let item = if has_yield {
        quote!(_)
    } else {
        quote!(::core::convert::Infallible)
    };
    Ok(quote!({
        let #state = #coroutine;
        unsafe {::nitori_call::__private::build::<#host,_,#item>(#state)}
    }))
}

pub(super) fn expand_function(attribute: Tokens, mut function: ItemFn) -> Result<Tokens> {
    #[derive(Default)]
    struct Options {
        sync: bool,
        yields: Option<Type>,
    }
    impl syn::parse::Parse for Options {
        fn parse(input: syn::parse::ParseStream<'_>) -> Result<Self> {
            let mut options = Self::default();
            while !input.is_empty() {
                let name: Ident = input.parse()?;
                if name == "sync" {
                    if options.sync {
                        return Err(Error::new(name.span(), "duplicate sync option"));
                    }
                    options.sync = true;
                } else if name == "yields" {
                    if options.yields.is_some() {
                        return Err(Error::new(name.span(), "duplicate yields option"));
                    }
                    input.parse::<Token![=]>()?;
                    options.yields = Some(input.parse()?);
                } else {
                    return Err(Error::new(name.span(), "expected sync or yields = Type"));
                }
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
            }
            Ok(options)
        }
    }
    let options = syn::parse2::<Options>(attribute)?;
    let has_yields = options.yields.is_some();
    let item_type = options
        .yields
        .unwrap_or_else(|| parse_quote!(::core::convert::Infallible));
    if function.sig.asyncness.take().is_none() {
        return Err(Error::new_spanned(
            &function.sig,
            "call definition must be async fn",
        ));
    }
    if function.sig.unsafety.is_some()
        || function.sig.constness.is_some()
        || function.sig.abi.is_some()
        || function.sig.variadic.is_some()
    {
        return Err(Error::new_spanned(
            &function.sig,
            "unsafe, const and extern call definitions are not supported",
        ));
    }
    let mut inputs = std::mem::take(&mut function.sig.inputs).into_iter();
    let first = inputs
        .next()
        .ok_or_else(|| Error::new_spanned(&function.sig, "missing receiver first parameter"))?;
    let FnArg::Typed(parameter) = first else {
        return Err(Error::new_spanned(first, "expected receiver parameter"));
    };
    if !matches!(&*parameter.pat, Pat::Ident(binding) if binding.by_ref.is_none() && binding.subpat.is_none())
    {
        return Err(Error::new_spanned(
            parameter.pat,
            "receiver parameter must be an identifier",
        ));
    }
    let receiver = &parameter.pat;
    let annotation = &parameter.ty;
    let host = host_type(annotation)?;
    function.sig.inputs = inputs.collect();
    // Ordinary function arguments imply their referents' outlives bounds. Keep
    // those bounds when the macro also names the arguments and coroutine state
    // in structs and extension traits, outside the function's implied context.
    struct InputBounds(Vec<WherePredicate>);
    impl VisitMut for InputBounds {
        fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
            if let Some(lifetime) = &reference.lifetime
                && lifetime.ident != "_"
            {
                let ty = &reference.elem;
                self.0.push(parse_quote!(#ty: #lifetime));
            }
            visit_mut::visit_type_reference_mut(self, reference);
        }
        fn visit_type_bare_fn_mut(&mut self, _: &mut TypeBareFn) {}
        fn visit_trait_bound_mut(&mut self, bound: &mut TraitBound) {
            if bound.lifetimes.is_none() {
                visit_mut::visit_trait_bound_mut(self, bound);
            }
        }
    }
    let mut bounds = InputBounds(Vec::new());
    for argument in &mut function.sig.inputs {
        bounds.visit_fn_arg_mut(argument);
    }
    for predicate in bounds.0 {
        if let WherePredicate::Type(predicate) = &predicate
            && let Type::Path(path) = &predicate.bounded_ty
            && path.qself.is_none()
            && let Some(parameter) = function
                .sig
                .generics
                .type_params_mut()
                .find(|parameter| path.path.is_ident(&parameter.ident))
        {
            parameter.bounds.extend(predicate.bounds.clone());
        } else {
            function
                .sig
                .generics
                .make_where_clause()
                .predicates
                .push(predicate);
        }
    }
    let arguments = &function.sig.inputs;
    let mut wrapper_arguments = arguments.clone();
    let mut names = Vec::new();
    for argument in &mut wrapper_arguments {
        let FnArg::Typed(argument) = argument else {
            return Err(Error::new_spanned(argument, "unexpected self"));
        };
        let Pat::Ident(binding) = &mut *argument.pat else {
            return Err(Error::new_spanned(
                &argument.pat,
                "real parameters must be identifiers",
            ));
        };
        if binding.by_ref.is_some() || binding.subpat.is_some() {
            return Err(Error::new_spanned(
                binding,
                "real parameters must be identifiers",
            ));
        }
        binding.mutability = None;
        names.push(binding.ident.clone());
    }
    let output: Type = match &function.sig.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, ty) => *ty.clone(),
    };
    let body = &function.block;
    let closure: ExprClosure = parse_quote!(move |#receiver: #annotation| -> #output #body);
    let (coroutine, _, _) = coroutine(closure)?;
    let name = &function.sig.ident;
    let call_name = Ident::new(&camel(&name.to_string()), name.span());
    let module = format_ident!("__call_{}", name);
    let visibility = &function.vis;
    let attributes = &function.attrs;
    let generics = &function.sig.generics;
    let (implementation, types, constraints) = generics.split_for_impl();
    // The public alias carries names only; the nominal Parameters type and
    // implementations enforce the original bounds.
    let mut alias_generics = generics.clone();
    alias_generics.where_clause = None;
    for parameter in &mut alias_generics.params {
        match parameter {
            GenericParam::Type(parameter) => parameter.bounds.clear(),
            GenericParam::Lifetime(parameter) => parameter.bounds.clear(),
            GenericParam::Const(_) => {}
        }
    }
    // Operation lives one module deeper than the user's declaration.
    let mut operation_visibility = visibility.clone();
    match &mut operation_visibility {
        syn::Visibility::Inherited => operation_visibility = parse_quote!(pub(super)),
        syn::Visibility::Restricted(restriction) if !restriction.path.is_ident("crate") => {
            let path = &restriction.path;
            restriction.path = if path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == "self")
            {
                let mut path = (**path).clone();
                path.segments.first_mut().unwrap().ident = Ident::new("super", name.span());
                Box::new(path)
            } else if path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == "super")
            {
                Box::new(parse_quote!(super::#path))
            } else {
                path.clone()
            };
            restriction.in_token = Some(Default::default());
        }
        _ => {}
    }

    // Lifetimes are inferred at make/new calls; types and const parameters must
    // be explicit because the receiver host does not occur in real arguments.
    let parameters: Vec<Tokens> = generics
        .params
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) => {
                let name = &parameter.ident;
                Some(quote!(#name))
            }
            GenericParam::Const(parameter) => {
                let name = &parameter.ident;
                Some(quote!(#name))
            }
            GenericParam::Lifetime(_) => None,
        })
        .collect();
    let visit = fresh_lifetime(generics, "__visit");
    let helpers = helper_trait(&function, &host, &call_name, options.sync, has_yields)?;
    Ok(quote! {
        #[doc(hidden)]
        #visibility mod #module {
            use super::*;
            pub type __State #implementation #constraints = impl ::core::ops::Coroutine<
                ::nitori_call::__private::ResumeEnv<#host>,
                Yield=__Yield #types, Return=__Return #types>;
            #[define_opaque(__State)]
            pub(super) fn make #implementation (#arguments) -> __State #types #constraints {
                ::nitori_call::__private::map_coroutine(#coroutine,
                    |value| -> __Yield #types { __Yield { value, marker: ::core::marker::PhantomData } },
                    |value| -> __Return #types { __Return { value, marker: ::core::marker::PhantomData } })
            }
            // Keep another operation's opaque state out of the __State TAIT's
            // associated Yield/Return constraints. A nominal boundary avoids rustc's
            // E0282 for TAIT bounds containing another opaque type.
            #operation_visibility struct __Return #implementation #constraints {
                pub(super) value: #output,
                marker: ::core::marker::PhantomData<fn() -> __Parameters #types>,
            }
            #operation_visibility struct __Yield #implementation #constraints {
                value: ::nitori_call::__private::Suspend<#item_type>,
                marker: ::core::marker::PhantomData<fn() -> __Parameters #types>,
            }
            impl #implementation ::nitori_call::__private::Suspension for __Yield #types #constraints {
                type Item = #item_type;
                fn into_suspend(self) -> ::nitori_call::__private::Suspend<Self::Item> { self.value }
            }
            // Keep user generics outside pin-project-lite's restricted parser.
            // Parameters makes every lifetime/type/const parameter nominally
            // present, including ones used only by the opaque coroutine state.
            pub struct __Parameters #implementation #constraints {
                marker: ::core::marker::PhantomData<fn() -> __State #types>,
            }
            ::nitori_call::__private::pin_project! {
                #operation_visibility struct __Operation<Parameters, Inner> {
                    #[pin]
                    pub(super) inner: Inner,
                    pub(super) marker: ::core::marker::PhantomData<fn() -> Parameters>,
                }
            }
        }
        #visibility type #call_name #alias_generics = #module::__Operation<
            #module::__Parameters #types,
            ::nitori_call::__private::StackCall<#host, #module::__State #types>,
        >;
        impl #implementation #call_name #types #constraints {
            #visibility fn new(#wrapper_arguments) -> Self {
                let state = #module::make::<#(#parameters),*>(#(#names),*);
                Self { inner: unsafe { ::nitori_call::__private::build_mapped(state) }, marker: ::core::marker::PhantomData }
            }
        }
        #(#attributes)*
        #visibility fn #name #implementation (#wrapper_arguments) -> #call_name #types #constraints {
            #call_name::<#(#parameters),*>::new(#(#names),*)
        }
        impl #implementation ::nitori_call::CallOn<#host> for #call_name #types #constraints {
            type Yield = #item_type;
            type Return = #output;
            #[allow(unreachable_code)]
            fn poll_call<#visit>(self: ::core::pin::Pin<&mut Self>, host: ::core::pin::Pin<&mut <#host as ::nitori_call::HostFamily>::Host<#visit>>, cx: &mut ::core::task::Context<'_>)
                -> ::core::task::Poll<::core::ops::CoroutineState<Self::Yield, Self::Return>> where #host: #visit {
                ::nitori_call::CallOn::poll_call(self.project().inner, host, cx).map(|event| match event {
                    ::core::ops::CoroutineState::Yielded(value) => ::core::ops::CoroutineState::Yielded(value),
                    ::core::ops::CoroutineState::Complete(value) => ::core::ops::CoroutineState::Complete(value.value),
                })
            }
        }
        #helpers
    })
}

fn helper_trait(
    function: &ItemFn,
    host: &Type,
    call_name: &Ident,
    sync: bool,
    has_yields: bool,
) -> Result<Tokens> {
    let name = &function.sig.ident;
    let unpin_name = format_ident!("{}_unpin", name);
    let sync_name = format_ident!("sync_{}", name);
    let sync_unpin_name = format_ident!("sync_{}_unpin", name);
    let trait_name = format_ident!("{}Ext", call_name);
    let receiver_trait = format_ident!("Receiver{}Ext", call_name);
    let arguments_name = format_ident!("{}Arguments", call_name);
    let visibility = &function.vis;
    let generics = &function.sig.generics;
    let (params, types, constraints) = generics.split_for_impl();
    let call_type = quote!(#call_name #types);
    let mut arguments = function.sig.inputs.clone();
    let mut names = Vec::new();
    let mut field_types = Vec::new();
    for argument in &mut arguments {
        if let FnArg::Typed(argument) = argument
            && let Pat::Ident(binding) = &mut *argument.pat
        {
            binding.mutability = None;
            names.push(binding.ident.clone());
            field_types.push(argument.ty.clone());
        }
    }
    let host_parameter = fresh_parameter(generics, "__CallHost");
    let route_parameter = fresh_parameter(generics, "__CallRoute");
    let call_lifetime = fresh_lifetime(generics, "__call");
    let mut marker_name = "__arguments_marker".to_owned();
    while names.iter().any(|name| *name == marker_name) {
        marker_name.push('_');
    }
    let marker = private(&marker_name);
    let mut host_generics = generics.clone();
    host_generics
        .params
        .push(parse_quote!(#host_parameter: ::nitori_call::Host<Family = #host> + ?Sized));
    let (host_params, _, host_constraints) = host_generics.split_for_impl();
    let mut route_generics = generics.clone();
    route_generics
        .params
        .push(parse_quote!(#route_parameter: ::nitori_call::Route<Target = #host>));
    let (route_params, _, route_constraints) = route_generics.split_for_impl();
    let synchronous = if sync {
        let (result, execute, bounds) = if has_yields {
            (
                quote!(::nitori_call::SyncBoundCall<#call_lifetime,Self,#call_type>),
                quote!(::nitori_call::SyncBoundCall::new),
                quote!(),
            )
        } else {
            (
                quote!(<#call_type as ::nitori_call::CallOn<#host>>::Return),
                quote!(::nitori_call::run_sync),
                quote!(#call_type: ::nitori_call::CallOn<#host,Yield=::core::convert::Infallible>,),
            )
        };
        quote! {
            fn #sync_name<#call_lifetime>(self: ::core::pin::Pin<&#call_lifetime mut Self>, #arguments) -> #result where #bounds {
                #execute(self, <#call_type>::new(#(#names),*))
            }
            fn #sync_unpin_name<#call_lifetime>(&#call_lifetime mut self, #arguments) -> #result where #bounds Self: Unpin {
                #execute(::core::pin::Pin::new(self), <#call_type>::new(#(#names),*))
            }
        }
    } else {
        quote!()
    };
    Ok(quote! {
        #visibility struct #arguments_name #params #constraints {
            #(pub #names: #field_types,)*
            #marker: ::core::marker::PhantomData<fn() -> #call_type>,
        }
        impl #params #arguments_name #types #constraints {
            #visibility fn new(#arguments) -> Self { Self { #(#names,)* #marker: ::core::marker::PhantomData } }
        }
        impl #params ::nitori_call::Arguments<#host> for #arguments_name #types #constraints {
            type Call = #call_type;
            fn into_call(self) -> Self::Call { <#call_type>::new(#(self.#names),*) }
        }
        #visibility trait #trait_name #params: ::nitori_call::Host<Family = #host> #constraints {
            #synchronous
            fn #name<#call_lifetime>(self: ::core::pin::Pin<&#call_lifetime mut Self>, #arguments) -> ::nitori_call::BoundCall<#call_lifetime,Self,#call_type> {
                ::nitori_call::BoundCall::new(self, <#call_type>::new(#(#names),*))
            }
            fn #unpin_name<#call_lifetime>(&#call_lifetime mut self, #arguments) -> ::nitori_call::BoundCall<#call_lifetime,Self,#call_type> where Self: Unpin {
                ::nitori_call::BoundCall::new(::core::pin::Pin::new(self), <#call_type>::new(#(#names),*))
            }
        }
        impl #host_params #trait_name #types for #host_parameter #host_constraints {}
        #visibility trait #receiver_trait #params: ::nitori_call::Route<Target = #host> + Sized #constraints {
            fn #name(self, #arguments) -> ::nitori_call::Child<Self::Root, ::nitori_call::Routed<Self,#call_type>> {
                ::nitori_call::ReceiverExt::call(self, <#arguments_name #types>::new(#(#names),*))
            }
        }
        impl #route_params #receiver_trait #types for #route_parameter #route_constraints {}
    })
}
