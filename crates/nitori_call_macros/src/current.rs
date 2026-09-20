use super::*;

// These names have definition-site local-variable hygiene. Source code cannot
// capture or shadow the environment, even by spelling the same identifier.
fn private(name: &str) -> Ident {
    Ident::new(name, Span::mixed_site())
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
        self.error = Some(Error::new_spanned(
            invocation,
            "opaque macros are outside the call macro safety boundary",
        ));
    }
}
struct Rewrite {
    receiver: Ident,
    host: Box<Type>,
    environment: Ident,
    has_yield: bool,
    error: Option<Error>,
}
impl Rewrite {
    fn direct(&self, expression: &Expr) -> bool {
        matches!(expression,Expr::Path(path) if path.path.is_ident(&self.receiver))
    }
    fn sync_chain(&self, expression: &Expr) -> bool {
        self.direct(expression)
            || matches!(expression,Expr::MethodCall(method) if method.args.is_empty() && self.sync_chain(&method.receiver))
    }
    fn access(&self, callback: Expr) -> Expr {
        let environment = &self.environment;
        let callback_name = private("__stack_callback");
        let host = &self.host;
        // Evaluate all author code OUTSIDE the generated unsafe block.
        parse_quote!({let #callback_name=::nitori_call::__private::prepare::<#host,_,_>(#callback);unsafe {#environment.with(#callback_name)}})
    }
    fn awaited(&self, input: Tokens, child: bool) -> Expr {
        let environment = &self.environment;
        let state = private("__stack_child");
        let poll = if child {
            quote!(#environment.poll_complete(#state.as_mut()))
        } else {
            quote!(#environment.poll_future(#state.as_mut()))
        };
        parse_quote!({
            let mut #state=::core::pin::pin!(#input);
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
        if let Expr::Await(awaited) = expression {
            if let Expr::MethodCall(method) = &*awaited.base
                && self.direct(&method.receiver)
            {
                if method.method == "with" {
                    self.error = Some(Error::new_spanned(
                        method,
                        "with is synchronous; omit await",
                    ));
                    return;
                }
                let name = Ident::new(&camel(&method.method.to_string()), method.method.span());
                let mut ty: TypePath = parse_quote!(#name);
                if let Some(arguments) = &method.turbofish {
                    ty.path.segments.last_mut().unwrap().arguments =
                        PathArguments::AngleBracketed(arguments.clone());
                }
                let constructor = constructor(&ty);
                let mut arguments = method.args.clone();
                for argument in &mut arguments {
                    self.visit_expr_mut(argument);
                }
                *expression = self.awaited(quote!(#constructor(#arguments)), true);
            } else {
                let mut future = (*awaited.base).clone();
                self.visit_expr_mut(&mut future);
                *expression = self.awaited(
                    quote!(::core::future::IntoFuture::into_future(#future)),
                    false,
                );
            }
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
        if let Expr::MethodCall(method) = expression
            && self.direct(&method.receiver)
            && method.method == "with"
        {
            if method.args.len() != 1 {
                self.error = Some(Error::new_spanned(
                    method,
                    "with expects one synchronous callback",
                ));
                return;
            }
            // A real closure parameter may use the author's io name. There is no
            // binding for the virtual io in the expansion, so it cannot capture
            // the hidden environment. Borrow checking validates the callback.
            let mut callback = method.args[0].clone();
            // The callback-producing expression runs in the parent coroutine and
            // can itself await/yield. Only an actual closure body is a new scope.
            self.visit_expr_mut(&mut callback);
            *expression = self.access(callback);
            return;
        }
        if let Expr::MethodCall(method) = expression
            && self.direct(&method.receiver)
        {
            let host = private("__stack_host");
            let method_name = &method.method;
            let turbofish = &method.turbofish;
            let mut arguments = method.args.clone();
            for argument in &mut arguments {
                self.visit_expr_mut(argument);
            }
            let names: Vec<_> = (0..arguments.len())
                .map(|index| private(&format!("__stack_argument_{index}")))
                .collect();
            let arguments: Vec<_> = arguments.into_iter().collect();
            let access = self.access(
                parse_quote!(move |#host| { #[allow(unused_mut)] let mut #host=#host; #host.#method_name #turbofish (#(#names),*) }),
            );
            *expression = parse_quote!({#(let #names=#arguments;)* #access});
            return;
        }
        if let Expr::MethodCall(method) = expression
            && method.args.is_empty()
            && self.sync_chain(&method.receiver)
        {
            let host = private("__stack_host");
            let mut invocation = expression.clone();
            struct Rebind<'a> {
                from: &'a Ident,
                to: &'a Ident,
            }
            impl VisitMut for Rebind<'_> {
                fn visit_expr_path_mut(&mut self, path: &mut ExprPath) {
                    if path.path.is_ident(self.from) {
                        let to = self.to;
                        path.path = parse_quote!(#to);
                    }
                }
            }
            Rebind {
                from: &self.receiver,
                to: &host,
            }
            .visit_expr_mut(&mut invocation);
            *expression = self.access(
                parse_quote!(|#host| { #[allow(unused_mut)] let mut #host=#host; #invocation }),
            );
            return;
        }
        if let Expr::Path(path) = expression
            && path.path.is_ident(&self.receiver)
        {
            self.error = Some(Error::new_spanned(
                path,
                "virtual host cannot escape call macro",
            ));
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }
    fn visit_pat_ident_mut(&mut self, pattern: &mut PatIdent) {
        if pattern.ident == self.receiver {
            self.error = Some(Error::new_spanned(
                pattern,
                "virtual host cannot be shadowed",
            ));
        }
    }
    // These have their own execution/suspension scope. Validation still examines
    // their tokens for opaque macros/attributes, but lowering leaves them intact.
    fn visit_expr_async_mut(&mut self, _: &mut ExprAsync) {}
    fn visit_expr_closure_mut(&mut self, _: &mut ExprClosure) {}
    fn visit_item_mut(&mut self, _: &mut Item) {}
}
// The virtual parameter describes the short borrow received by with, not an
// owned host captured by the operation. Resolve only this explicit syntax.
fn host_type(annotation: &Type) -> Result<Box<Type>> {
    let invalid = || {
        Error::new_spanned(
            annotation,
            "virtual host type must be Pin<&mut T> with an elided borrow lifetime",
        )
    };
    let Type::Path(path) = annotation else {
        return Err(invalid());
    };
    let segment = path.path.segments.last().ok_or_else(invalid)?;
    if path.qself.is_some() || segment.ident != "Pin" {
        return Err(invalid());
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(invalid());
    };
    if arguments.args.len() != 1 {
        return Err(invalid());
    }
    let Some(GenericArgument::Type(Type::Reference(reference))) = arguments.args.first() else {
        return Err(invalid());
    };
    if reference.mutability.is_none() || reference.lifetime.is_some() {
        return Err(invalid());
    }
    let mut host = reference.elem.as_ref();
    while let Type::Paren(parenthesized) = host {
        host = parenthesized.elem.as_ref();
    }
    Ok(Box::new(host.clone()))
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
            "expected |io: Pin<&mut T>| { ... }",
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
        closure.inputs[0] = Pat::Type(parse_quote!(#binding: ::core::pin::Pin<&mut _>));
    }
    let Some(Pat::Type(parameter)) = closure.inputs.first() else {
        return Err(Error::new_spanned(
            &closure.inputs,
            "expected a virtual host identifier",
        ));
    };
    let Pat::Ident(binding) = &*parameter.pat else {
        return Err(Error::new_spanned(parameter, "host identifier required"));
    };
    if binding.by_ref.is_some() || binding.subpat.is_some() {
        return Err(Error::new_spanned(
            binding,
            "virtual parameter must be a plain identifier",
        ));
    }
    let receiver = binding.ident.clone();
    let host = host_type(&parameter.ty)?;
    let environment = private("__stack_environment");
    let mut rewrite = Rewrite {
        receiver,
        host: host.clone(),
        environment: environment.clone(),
        has_yield: false,
        error: None,
    };
    rewrite.visit_expr_mut(&mut closure.body);
    if let Some(error) = rewrite.error {
        return Err(error);
    }
    let body = match *closure.body {
        Expr::Block(block) => quote!(#block),
        expression => quote!({#expression}),
    };
    let capture = closure.capture;
    let output = closure.output;
    Ok((
        quote!(::core::convert::identity(#[coroutine] static #capture |mut #environment: ::nitori_call::__private::ResumeEnv<#host>| #output #body)),
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
        .ok_or_else(|| Error::new_spanned(&function.sig, "missing virtual first parameter"))?;
    let FnArg::Typed(parameter) = first else {
        return Err(Error::new_spanned(first, "expected virtual parameter"));
    };
    if !matches!(&*parameter.pat, Pat::Ident(binding) if binding.by_ref.is_none() && binding.subpat.is_none())
    {
        return Err(Error::new_spanned(
            parameter.pat,
            "virtual parameter must be an identifier",
        ));
    }
    let receiver = &parameter.pat;
    let annotation = &parameter.ty;
    let host = host_type(annotation)?;
    function.sig.inputs = inputs.collect();
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
    // be explicit because the virtual host does not occur in real arguments.
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
    let helpers = helper_trait(&function, &host, &call_name, options.sync, has_yields)?;
    Ok(quote! {
        #[doc(hidden)]
        #visibility mod #module {
            use super::*;
            pub type State #implementation #constraints = impl ::core::ops::Coroutine<
                ::nitori_call::__private::ResumeEnv<#host>,
                Yield=::nitori_call::__private::Suspend<#item_type>, Return=#output>;
            #[define_opaque(State)]
            pub(super) fn make #implementation (#arguments) -> State #types #constraints {
                #coroutine
            }
            // Keep user generics outside pin-project-lite's restricted parser.
            // Parameters makes every lifetime/type/const parameter nominally
            // present, including ones used only by the opaque coroutine state.
            pub struct __Parameters #implementation #constraints {
                marker: ::core::marker::PhantomData<fn() -> State #types>,
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
            ::nitori_call::__private::StackCall<#host, #module::State #types>,
        >;
        impl #implementation #call_name #types #constraints {
            #visibility fn new(#wrapper_arguments) -> Self {
                let state = #module::make::<#(#parameters),*>(#(#names),*);
                Self { inner: unsafe { ::nitori_call::__private::build(state) }, marker: ::core::marker::PhantomData }
            }
        }
        #(#attributes)*
        #visibility fn #name #implementation (#wrapper_arguments) -> #call_name #types #constraints {
            #call_name::<#(#parameters),*>::new(#(#names),*)
        }
        impl #implementation ::nitori_call::CallOn<#host> for #call_name #types #constraints {
            type Yield = #item_type;
            type Return = #output;
            fn poll_call(self: ::core::pin::Pin<&mut Self>, host: ::core::pin::Pin<&mut #host>, cx: &mut ::core::task::Context<'_>)
                -> ::core::task::Poll<::core::ops::CoroutineState<Self::Yield, Self::Return>> {
                ::nitori_call::CallOn::poll_call(self.project().inner, host, cx)
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
    let trait_name = format_ident!("{}Ext", call_name);
    let visibility = &function.vis;
    let host_parameter = function.sig.generics.type_params().find(|parameter| {
        matches!(host, Type::Path(path) if path.qself.is_none() && path.path.is_ident(&parameter.ident))
    });
    struct Substitute<'a>(Option<&'a Ident>);
    impl VisitMut for Substitute<'_> {
        fn visit_path_mut(&mut self, path: &mut Path) {
            if let Some(parameter) = self.0
                && let Some(first) = path.segments.first_mut()
                && first.ident == *parameter
            {
                first.ident = Ident::new("Self", first.ident.span());
            }
            visit_mut::visit_path_mut(self, path);
        }
    }
    let mut substitute = Substitute(host_parameter.map(|parameter| &parameter.ident));
    let mut methods = function.sig.generics.clone();
    if let Some(parameter) = host_parameter {
        methods.params = methods.params.into_iter().filter(|candidate| {
            !matches!(candidate, GenericParam::Type(candidate) if candidate.ident == parameter.ident)
        }).collect();
        let mut bounds = parameter.bounds.clone();
        if let Some(clause) = &methods.where_clause {
            for predicate in &clause.predicates {
                if let WherePredicate::Type(predicate) = predicate
                    && matches!(&predicate.bounded_ty, Type::Path(path) if path.path.is_ident(&parameter.ident))
                {
                    bounds.extend(predicate.bounds.clone());
                }
            }
        }
        let relaxed = bounds.iter().any(|bound| matches!(bound, TypeParamBound::Trait(bound) if matches!(bound.modifier, TraitBoundModifier::Maybe(_)) && bound.path.is_ident("Sized")));
        bounds = bounds.into_iter().filter(|bound| !matches!(bound, TypeParamBound::Trait(bound) if matches!(bound.modifier, TraitBoundModifier::Maybe(_)))).collect();
        if !bounds.is_empty() {
            methods
                .make_where_clause()
                .predicates
                .push(parse_quote!(Self: #bounds));
        }
        if !relaxed {
            methods
                .make_where_clause()
                .predicates
                .push(parse_quote!(Self: Sized));
        }
    }
    substitute.visit_generics_mut(&mut methods);
    // Relaxed Sized belongs on a type parameter declaration, not on trait Self.
    if let Some(clause) = &mut methods.where_clause {
        for predicate in &mut clause.predicates {
            if let WherePredicate::Type(predicate) = predicate {
                predicate.bounds = predicate.bounds.clone().into_iter().filter(|bound| !matches!(bound, TypeParamBound::Trait(bound) if matches!(bound.modifier, TraitBoundModifier::Maybe(_)))).collect();
            }
        }
        clause.predicates = clause.predicates.clone().into_iter().filter(|predicate| !matches!(predicate, WherePredicate::Type(predicate) if predicate.bounds.is_empty())).collect();
    }
    let mut arguments = function.sig.inputs.clone();
    for argument in &mut arguments {
        substitute.visit_fn_arg_mut(argument);
    }
    let mut names = Vec::new();
    for argument in &mut arguments {
        if let FnArg::Typed(argument) = argument
            && let Pat::Ident(binding) = &mut *argument.pat
        {
            binding.mutability = None;
            names.push(binding.ident.clone());
        }
    }
    let parameters: Vec<Tokens> = function
        .sig
        .generics
        .params
        .iter()
        .map(|parameter| match parameter {
            GenericParam::Type(parameter)
                if host_parameter.is_some_and(|host| host.ident == parameter.ident) =>
            {
                quote!(Self)
            }
            GenericParam::Type(parameter) => {
                let name = &parameter.ident;
                quote!(#name)
            }
            GenericParam::Lifetime(parameter) => {
                let lifetime = &parameter.lifetime;
                quote!(#lifetime)
            }
            GenericParam::Const(parameter) => {
                let name = &parameter.ident;
                quote!(#name)
            }
        })
        .collect();
    let call_type: Type = if parameters.is_empty() {
        parse_quote!(#call_name)
    } else {
        parse_quote!(#call_name<#(#parameters),*>)
    };
    methods
        .make_where_clause()
        .predicates
        .push(parse_quote!(#call_type: ::nitori_call::CallOn<Self>));
    // Operation parameters belong on the trait so its signature can name the
    // complete operation and all capability requirements.
    let trait_generics = methods;
    let (trait_parameters, trait_arguments, trait_constraints) = trait_generics.split_for_impl();
    let mut host_name = "__CallHost".to_owned();
    while trait_generics
        .type_params()
        .any(|parameter| parameter.ident == host_name)
    {
        host_name.push('_');
    }
    let implementation_host = Ident::new(&host_name, Span::mixed_site());
    struct ReplaceSelf<'a>(&'a Ident);
    impl VisitMut for ReplaceSelf<'_> {
        fn visit_path_mut(&mut self, path: &mut Path) {
            if let Some(first) = path.segments.first_mut()
                && first.ident == "Self"
            {
                first.ident = self.0.clone();
            }
            visit_mut::visit_path_mut(self, path);
        }
    }
    let mut implementation = trait_generics.clone();
    ReplaceSelf(&implementation_host).visit_generics_mut(&mut implementation);
    implementation
        .params
        .push(parse_quote!(#implementation_host: ?Sized));
    let (impl_parameters, _, impl_constraints) = implementation.split_for_impl();
    let mut lifetime_name = "__call_host".to_owned();
    while trait_generics
        .lifetimes()
        .any(|parameter| parameter.lifetime.ident == lifetime_name)
    {
        lifetime_name.push('_');
    }
    let lifetime = Lifetime::new(&format!("'{lifetime_name}"), Span::mixed_site());
    let sync_methods = if sync {
        let sync_name = format_ident!("sync_{}", name);
        let sync_unpin_name = format_ident!("sync_{}_unpin", name);
        let (result, execute) = if has_yields {
            (
                quote!(::nitori_call::SyncBoundCall<#lifetime, Self, #call_type>),
                quote!(::nitori_call::SyncBoundCall::new),
            )
        } else {
            (
                quote!(<#call_type as ::nitori_call::CallOn<Self>>::Return),
                quote!(::nitori_call::run_sync),
            )
        };
        let bounds = if has_yields {
            quote!()
        } else {
            quote!(#call_type: ::nitori_call::CallOn<Self, Yield = ::core::convert::Infallible>,)
        };
        quote! {
            fn #sync_name<#lifetime>(self: ::core::pin::Pin<&#lifetime mut Self>, #arguments)
                -> #result where #bounds {
                #execute(self, <#call_type>::new(#(#names),*))
            }
            fn #sync_unpin_name<#lifetime>(&#lifetime mut self, #arguments)
                -> #result where #bounds Self: ::core::marker::Unpin {
                #execute(::core::pin::Pin::new(self), <#call_type>::new(#(#names),*))
            }
        }
    } else {
        quote!()
    };
    Ok(quote! {
        #visibility trait #trait_name #trait_parameters #trait_constraints {
            #sync_methods
            fn #name<#lifetime>(self: ::core::pin::Pin<&#lifetime mut Self>, #arguments)
                -> ::nitori_call::BoundCall<#lifetime, Self, #call_type> {
                ::nitori_call::BoundCall::new(self, <#call_type>::new(#(#names),*))
            }
            fn #unpin_name<#lifetime>(&#lifetime mut self, #arguments)
                -> ::nitori_call::BoundCall<#lifetime, Self, #call_type>
                where Self: ::core::marker::Unpin {
                ::nitori_call::BoundCall::new(::core::pin::Pin::new(self), <#call_type>::new(#(#names),*))
            }
        }
        impl #impl_parameters #trait_name #trait_arguments for #implementation_host #impl_constraints {}
    })
}
