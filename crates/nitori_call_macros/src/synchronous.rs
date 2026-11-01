//! `#[capability]` for synchronous methods: those taking a view but no
//! context, which become methods of lent accesses and of views.
use crate::{Argument, SelfAsFamily, arguments, mentions, runtime};
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::{
    Attribute, Error, FnArg, GenericArgument, GenericParam, Generics, Ident, ItemTrait, Lifetime,
    Path, PathArguments, Result, ReturnType, TraitItem, Type, TypeParamBound, WherePredicate,
    parse_quote, spanned::Spanned, visit_mut::VisitMut,
};

/// The lifetimes a view parameter `Pin<&'b mut Self::ReceiverView<'v>>` names.
struct ViewLifetimes {
    borrow: Option<Lifetime>,
    view: Option<Lifetime>,
}
impl ViewLifetimes {
    /// Parse `ty`; `None` when it is not a view parameter.
    fn of(ty: &Type) -> Option<Self> {
        let Type::Path(pin) = ty else {
            return None;
        };
        let last = pin.path.segments.last()?;
        let PathArguments::AngleBracketed(arguments) = &last.arguments else {
            return None;
        };
        let (true, Some(GenericArgument::Type(Type::Reference(reference)))) =
            (last.ident == "Pin", arguments.args.first())
        else {
            return None;
        };
        let (Some(_), Type::Path(view)) = (&reference.mutability, &*reference.elem) else {
            return None;
        };
        let last = view.path.segments.last()?;
        let of_self = match &view.qself {
            Some(qself) => matches!(&*qself.ty, Type::Path(owner) if owner.path.is_ident("Self")),
            None => view.path.segments.first()?.ident == "Self",
        };
        if !of_self || last.ident != "ReceiverView" {
            return None;
        }
        let view = match &last.arguments {
            PathArguments::AngleBracketed(arguments) => {
                arguments.args.iter().find_map(|argument| match argument {
                    GenericArgument::Lifetime(lifetime) if lifetime.ident != "_" => {
                        Some(lifetime.clone())
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        // No lent view lives that long.
        if view.as_ref().is_some_and(|view| view.ident == "static") {
            return None;
        }
        let borrow = reference
            .lifetime
            .clone()
            .filter(|lifetime| lifetime.ident != "_");
        Some(Self { borrow, view })
    }
    fn names(&self) -> Vec<String> {
        [&self.borrow, &self.view]
            .into_iter()
            .flatten()
            .map(Lifetime::to_string)
            .collect()
    }
}

/// A method of a capability that takes a view and is not a `poll_` method.
pub(crate) struct SynchronousMethod {
    pub(crate) name: Ident,
    docs: Vec<Attribute>,
    generics: Generics,
    lifetimes: ViewLifetimes,
    arguments: Vec<Argument>,
    output: ReturnType,
}

/// The synchronous methods of `item`.
pub(crate) fn methods(item: &ItemTrait) -> Result<Vec<SynchronousMethod>> {
    let mut methods = Vec::new();
    for member in &item.items {
        let TraitItem::Fn(method) = member else {
            continue;
        };
        let signature = &method.sig;
        let plain = signature.asyncness.is_none() && signature.unsafety.is_none();
        if !plain || signature.ident.to_string().starts_with("poll_") {
            continue;
        }
        let mut inputs = signature.inputs.iter();
        let Some(FnArg::Typed(first)) = inputs.next() else {
            continue;
        };
        let Some(lifetimes) = ViewLifetimes::of(&first.ty) else {
            continue;
        };
        if lifetimes.borrow.is_some() && lifetimes.borrow == lifetimes.view {
            return Err(Error::new(
                first.ty.span(),
                "a view cannot be borrowed for its own lifetime; give the borrow a lifetime \
                 of its own",
            ));
        }
        methods.push(SynchronousMethod {
            name: signature.ident.clone(),
            docs: method
                .attrs
                .iter()
                .filter(|attribute| attribute.path().is_ident("doc"))
                .cloned()
                .collect(),
            generics: signature.generics.clone(),
            lifetimes,
            // The view comes first.
            arguments: arguments(inputs, 1)?,
            output: signature.output.clone(),
        });
    }
    Ok(methods)
}

/// Renames the lifetime of a view to that of the `Access` holding it.
struct ViewLifetimeAs<'a> {
    view: Option<&'a Lifetime>,
    access: &'a Lifetime,
}
impl VisitMut for ViewLifetimeAs<'_> {
    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if self.view == Some(lifetime) {
            *lifetime = self.access.clone();
        }
    }
}

/// Names of a method's type and const parameters, for a turbofish: one of them
/// may appear only in the output.
fn turbofish(generics: &Generics) -> Option<Tokens> {
    let names: Vec<&Ident> = generics
        .params
        .iter()
        .filter_map(|parameter| match parameter {
            GenericParam::Type(parameter) => Some(&parameter.ident),
            GenericParam::Const(parameter) => Some(&parameter.ident),
            GenericParam::Lifetime(_) => None,
        })
        .collect();
    (!names.is_empty()).then(|| quote!(::<#(#names),*>))
}

impl SynchronousMethod {
    fn names(&self) -> Vec<&Ident> {
        self.arguments
            .iter()
            .map(|argument| &argument.name)
            .collect()
    }

    /// Whether `predicate` only says that the family outlives the view, as
    /// every capability method must.
    fn family_outlives_view(&self, predicate: &WherePredicate) -> bool {
        let names = self.lifetimes.names();
        let WherePredicate::Type(predicate) = predicate else {
            return false;
        };
        let bounded = &predicate.bounded_ty;
        mentions(quote!(#bounded), "Self")
            && predicate.bounds.iter().all(|bound| match bound {
                TypeParamBound::Lifetime(lifetime) => names.contains(&lifetime.to_string()),
                _ => false,
            })
    }

    /// The method's parameters and where clause, without the view's lifetimes
    /// and the requirement that the family outlives them.
    fn generics_without_view(&self) -> (Generics, Vec<WherePredicate>) {
        let names = self.lifetimes.names();
        let mut generics = self.generics.clone();
        generics.params = generics
            .params
            .into_iter()
            .filter(|parameter| match parameter {
                GenericParam::Lifetime(parameter) => {
                    !names.contains(&parameter.lifetime.to_string())
                }
                _ => true,
            })
            .collect();
        let predicates = generics
            .where_clause
            .take()
            .into_iter()
            .flat_map(|clause| clause.predicates)
            .filter(|predicate| !self.family_outlives_view(predicate))
            .collect();
        (generics, predicates)
    }

    /// Whether anything else in the signature needs the view's lifetimes, so
    /// that the method cannot be called through an access, whose view stays
    /// inside a closure. A closure's access says nothing about how those
    /// lifetimes relate, and an `impl Trait` result captures them all.
    fn needs_view(&self) -> bool {
        let names = self.lifetimes.names();
        let related = self.generics.lifetimes().any(|parameter| {
            names.contains(&parameter.lifetime.to_string()) && !parameter.bounds.is_empty()
        });
        let types: Vec<&Type> = self.arguments.iter().map(|argument| &argument.ty).collect();
        let output = &self.output;
        let (generics, predicates) = self.generics_without_view();
        let parameters = &generics.params;
        let signature = quote!(#parameters #(#predicates)* #(#types)* #output);
        related
            || mentions(quote!(#output), "impl")
            || names
                .iter()
                .any(|lifetime| mentions(signature.clone(), lifetime))
    }

    /// The method on a lent access, which goes through `LentAccess::with`,
    /// when it can have one.
    pub(crate) fn access_method(&self, capability: &Path) -> Option<Tokens> {
        if self.needs_view() {
            return None;
        }
        let runtime = runtime();
        let family: Type = parse_quote!(<Self as #runtime::ReceiverAccess>::Family);
        let (mut generics, mut predicates) = self.generics_without_view();
        let mut types: Vec<Type> = self
            .arguments
            .iter()
            .map(|argument| argument.ty.clone())
            .collect();
        let mut output = self.output.clone();
        let mut self_as_family = SelfAsFamily {
            family: &family,
            capability: Some(capability),
        };
        self_as_family.visit_generics_mut(&mut generics);
        for predicate in &mut predicates {
            self_as_family.visit_where_predicate_mut(predicate);
        }
        for ty in &mut types {
            self_as_family.visit_type_mut(ty);
        }
        self_as_family.visit_return_type_mut(&mut output);
        let SynchronousMethod { name, docs, .. } = self;
        let names = self.names();
        let parameters = &generics.params;
        let turbofish = turbofish(&generics);
        Some(quote! {
            #(#docs)*
            #[inline]
            #[track_caller]
            fn #name<#parameters>(&mut self, #(#names: #types),*) #output
            where
                Self: #runtime::LentAccess + ::core::marker::Sized,
                #(#predicates,)*
            {
                #runtime::LentAccess::with(self, move |__access| {
                    <#family as #capability>::#name #turbofish(__access.into_pin(), #(#names),*)
                })
            }
        })
    }

    /// The declaration and the implementation of the method on a view.
    fn view_method(&self, family: &Type, capability: &Path, access: &Lifetime) -> (Tokens, Tokens) {
        let view = self.lifetimes.view.as_ref();
        let mut generics = self.generics.clone();
        generics.params = generics
            .params
            .into_iter()
            .filter(|parameter| match parameter {
                GenericParam::Lifetime(parameter) => Some(&parameter.lifetime) != view,
                _ => true,
            })
            .collect();
        let mut types: Vec<Type> = self
            .arguments
            .iter()
            .map(|argument| argument.ty.clone())
            .collect();
        let mut output = self.output.clone();
        let mut self_as_family = SelfAsFamily {
            family,
            capability: Some(capability),
        };
        let mut rename = ViewLifetimeAs { view, access };
        self_as_family.visit_generics_mut(&mut generics);
        rename.visit_generics_mut(&mut generics);
        for ty in &mut types {
            self_as_family.visit_type_mut(ty);
            rename.visit_type_mut(ty);
        }
        self_as_family.visit_return_type_mut(&mut output);
        rename.visit_return_type_mut(&mut output);
        let SynchronousMethod { name, docs, .. } = self;
        let names = self.names();
        let parameters = &generics.params;
        let where_clause = &generics.where_clause;
        let borrow = &self.lifetimes.borrow;
        let signature = quote! {
            fn #name<#parameters>(&#borrow mut self, #(#names: #types),*) #output #where_clause
        };
        let turbofish = turbofish(&generics);
        (
            quote! {
                #(#docs)*
                #signature;
            },
            quote! {
                #[inline]
                #signature {
                    <#family as #capability>::#name #turbofish(self.as_pin(), #(#names),*)
                }
            },
        )
    }
}

/// The synchronous methods on an `Access` of the family: `{Trait}ViewExt`, over
/// the view's lifetime, the capability's parameters and the family.
pub(crate) fn view_trait(
    item: &ItemTrait,
    trait_generics: &Generics,
    capability: &Path,
    bounds: &Tokens,
    methods: &[SynchronousMethod],
) -> Tokens {
    if methods.is_empty() {
        return Tokens::new();
    }
    let runtime = runtime();
    let vis = &item.vis;
    let name = format_ident!("{}ViewExt", item.ident);
    let family = format_ident!("__F");
    let family_type: Type = parse_quote!(#family);
    let access = Lifetime::new("'__h", Span::call_site());
    let (declarations, implementations): (Vec<_>, Vec<_>) = methods
        .iter()
        .map(|method| method.view_method(&family_type, capability, &access))
        .unzip();
    let mut generics = trait_generics.clone();
    if let Some(clause) = &mut generics.where_clause {
        SelfAsFamily {
            family: &family_type,
            capability: Some(capability),
        }
        .visit_where_clause_mut(clause);
    }
    let parameters: Vec<&GenericParam> = generics.params.iter().collect();
    let arguments = generics.params.iter().map(|parameter| match parameter {
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
    });
    let where_clause = &generics.where_clause;
    let doc = format!(
        "The synchronous methods of [`{}`] on the view of an `Access`.",
        item.ident
    );
    quote! {
        #[doc = #doc]
        #vis trait #name<#access, #(#parameters,)* #family: #bounds + #access> #where_clause {
            #(#declarations)*
        }
        impl<'__a, #access, #(#parameters,)* #family: #bounds + #access>
            #name<#access, #(#arguments,)* #family> for #runtime::Access<'__a, #access, #family>
            #where_clause
        {
            #(#implementations)*
        }
    }
}
