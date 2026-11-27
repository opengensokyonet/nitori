//! `#[derive(MapIo)]`: replace the IO a value keeps.
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Error, Fields, GenericParam, Ident, Member, Result, Token, Type,
    WherePredicate,
    parse::{Parse, ParseStream},
    parse_quote,
    spanned::Spanned,
    visit::{self, Visit},
};

/// Where the IO is: the type parameter that is the IO, or the type
/// parameter of a field that keeps it (`within = P`).
enum Place {
    Io(Ident),
    Within(Ident),
}

impl Parse for Place {
    fn parse(input: ParseStream) -> Result<Self> {
        let first: Ident = input.parse()?;
        if input.peek(Token![=]) {
            if first != "within" {
                return Err(Error::new(
                    first.span(),
                    "expected `within = P` or a type parameter",
                ));
            }
            input.parse::<Token![=]>()?;
            Ok(Self::Within(input.parse()?))
        } else {
            Ok(Self::Io(first))
        }
    }
}

impl Place {
    fn param(&self) -> &Ident {
        match self {
            Self::Io(param) | Self::Within(param) => param,
        }
    }
}

/// Whether a type names the parameter.
struct Mentions<'a> {
    param: &'a Ident,
    found: bool,
}
impl<'ast> Visit<'ast> for Mentions<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.leading_colon.is_none()
            && path
                .segments
                .first()
                .is_some_and(|s| s.ident == *self.param)
        {
            self.found = true;
        }
        visit::visit_path(self, path);
    }
}
fn mentions(ty: &Type, param: &Ident) -> bool {
    let mut visitor = Mentions {
        param,
        found: false,
    };
    visitor.visit_type(ty);
    visitor.found
}
fn is_param(ty: &Type, param: &Ident) -> bool {
    matches!(ty, Type::Path(path) if path.qself.is_none() && path.path.is_ident(param))
}

pub fn expand(input: DeriveInput) -> Result<Tokens> {
    let attribute = input
        .attrs
        .iter()
        .find(|a| a.path().is_ident("map_io"))
        .ok_or_else(|| {
            Error::new(
                input.ident.span(),
                "name the IO: `#[map_io(S)]` or `#[map_io(within = P)]`",
            )
        })?;
    let place: Place = attribute.parse_args()?;
    let param = place.param();
    // The parameter is replaced by any type: it takes no bounds.
    let declared = input
        .generics
        .type_params()
        .find(|p| p.ident == *param)
        .ok_or_else(|| Error::new(param.span(), "not a type parameter of this struct"))?;
    let bounded_in_where = input
        .generics
        .where_clause
        .iter()
        .flat_map(|w| &w.predicates)
        .any(|p| matches!(p, WherePredicate::Type(t) if is_param(&t.bounded_ty, param)));
    if !declared.bounds.is_empty() || bounded_in_where {
        return Err(Error::new(
            param.span(),
            "the IO parameter is replaced by any type, so it takes no bounds",
        ));
    }
    let runtime = quote!(::nitori_codec::staged::MapIo);
    let u = format_ident!("__MapIoTo");
    let f = format_ident!("__map");
    let (source, with_param): (Tokens, Tokens) = match &place {
        Place::Io(_) => (quote!(#param), quote!(#u)),
        Place::Within(_) => (
            quote!(<#param as #runtime>::Io),
            quote!(<#param as #runtime>::With<#u>),
        ),
    };
    // How the one field that keeps the IO is replaced.
    let map_field = |ty: &Type, value: Tokens| -> Result<Tokens> {
        match &place {
            Place::Io(_) if is_param(ty, param) => Ok(quote!(#f(#value))),
            Place::Io(_) => Ok(quote!(<#ty as #runtime>::map_io(#value, #f))),
            Place::Within(_) if is_param(ty, param) => {
                Ok(quote!(<#param as #runtime>::map_io(#value, #f)))
            }
            Place::Within(_) => Err(Error::new(
                ty.span(),
                "with `within = P`, the field's type is `P`",
            )),
        }
    };
    // The fields of a struct or variant, rebuilt with the IO replaced.
    let rebuild = |path: Tokens,
                   fields: &Fields,
                   span: proc_macro2::Span|
     -> Result<(Tokens, Tokens)> {
        let members: Vec<(Member, &Type)> = match fields {
            Fields::Named(named) => named
                .named
                .iter()
                .map(|f| (Member::Named(f.ident.clone().unwrap()), &f.ty))
                .collect(),
            Fields::Unnamed(unnamed) => unnamed
                .unnamed
                .iter()
                .enumerate()
                .map(|(i, f)| (Member::from(i), &f.ty))
                .collect(),
            Fields::Unit => Vec::new(),
        };
        let keeping: Vec<_> = members
            .iter()
            .filter(|(_, ty)| mentions(ty, param))
            .collect();
        let [(source_member, source_ty)] = keeping.as_slice() else {
            return Err(Error::new(
                span,
                "the IO parameter must appear in exactly one field of each struct or variant: a variant without it would drop the IO",
            ));
        };
        let binding = |m: &Member| {
            format_ident!(
                "__field_{}",
                match m {
                    Member::Named(i) => i.to_string(),
                    Member::Unnamed(i) => i.index.to_string(),
                }
            )
        };
        let bindings = members.iter().map(|(m, _)| {
            let b = binding(m);
            quote!(#m: #b)
        });
        let pattern = quote!(#path { #(#bindings),* });
        let mut built = Vec::new();
        for (member, _) in &members {
            let b = binding(member);
            if member == source_member {
                let mapped = map_field(source_ty, quote!(#b))?;
                built.push(quote!(#member: #mapped));
            } else {
                built.push(quote!(#member: #b));
            }
        }
        Ok((pattern, quote!(#path { #(#built),* })))
    };
    let name = &input.ident;
    let body = match &input.data {
        Data::Struct(data) => {
            let (pattern, built) = rebuild(quote!(#name), &data.fields, name.span())?;
            quote!(let #pattern = self; #built)
        }
        Data::Enum(data) => {
            let mut arms = Vec::new();
            for variant in &data.variants {
                let v = &variant.ident;
                let (pattern, built) = rebuild(quote!(#name::#v), &variant.fields, v.span())?;
                arms.push(quote!(#pattern => #built));
            }
            quote!(match self { #(#arms),* })
        }
        Data::Union(_) => {
            return Err(Error::new(
                name.span(),
                "MapIo is derived for structs and enums",
            ));
        }
    };

    let with_args = input.generics.params.iter().map(|p| match p {
        GenericParam::Lifetime(l) => {
            let l = &l.lifetime;
            quote!(#l)
        }
        GenericParam::Type(t) if t.ident == *param => with_param.clone(),
        GenericParam::Type(t) => {
            let t = &t.ident;
            quote!(#t)
        }
        GenericParam::Const(c) => {
            let c = &c.ident;
            quote!(#c)
        }
    });
    let mut generics = input.generics.clone();
    if let Place::Within(_) = &place {
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#param: #runtime));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #runtime for #name #ty_generics #where_clause {
            type Io = #source;
            type With<#u> = #name<#(#with_args),*>;
            fn map_io<#u>(self, #f: impl ::core::ops::FnOnce(Self::Io) -> #u) -> Self::With<#u> {
                #body
            }
        }
    })
}
