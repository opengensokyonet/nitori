//! `#[derive(Numbered)]`: the type-level number of a component, or of any
//! type a generic component is generic over, for `nitori_driver`.
//!
//! The number of a type is a 64-bit hash of the crate's name, its name and
//! the place it is defined (file, line and column), with the place of every
//! macro expansion it came from: FNV-1a, then
//! the final mix of splitmix64, so names close to each other differ in
//! their high bits. It is written as a list of bits, the highest first,
//! that `nitori_driver` compares by trait resolution. A generic type mixes
//! in the numbers of its type parameters, each rotated by its position, so
//! that `Pair<A, B>` and `Pair<B, A>` differ, and so do `Wrap<Wrap<X>>` and
//! `X`; every type parameter must be `Numbered` itself. Lifetimes are
//! left out; a const parameter is refused.
//!
//! A type derived in the body of a `macro_rules!` macro has its name where
//! the macro is defined; the places of the expansions it came from tell
//! the types of two expansions apart.
//!
//! Two types of one crate given the same number are refused where the
//! second is derived. Across crates nothing is checked: two numbers are
//! the same by chance about once in 2⁶⁴.
#![feature(proc_macro_span)]
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{Mutex, OnceLock},
};

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, GenericParam, parse_macro_input, parse_quote};

/// The type-level number of a type: `nitori_driver::id::Numbered`; and,
/// the type being a component, what `At` makes of the node that has it:
/// `nitori_driver::need::AtTarget`, the one node.
#[proc_macro_derive(Numbered)]
pub fn derive_numbered(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match numbered(input) {
        Ok(out) => out.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn numbered(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let crate_name = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let place = place(&crate_name, name);
    let own = mix(fnv1a(&place));
    check_unique(own, &place, name)?;

    let mut generics = input.generics.clone();
    let mut params = Vec::new();
    for p in &input.generics.params {
        match p {
            GenericParam::Type(t) => params.push(t.ident.clone()),
            GenericParam::Lifetime(_) => {}
            GenericParam::Const(c) => {
                return Err(syn::Error::new_spanned(
                    c,
                    "a const parameter has no number: `Numbered` takes types and lifetimes only",
                ));
            }
        }
    }
    let bits = bits(own);
    let number = if params.is_empty() {
        bits
    } else {
        let numbers = quote! {
            ::nitori_driver::frunk_core::HList![
                #(<#params as ::nitori_driver::id::Numbered>::Number),*
            ]
        };
        let where_clause = generics.make_where_clause();
        for t in &params {
            where_clause
                .predicates
                .push(parse_quote!(#t: ::nitori_driver::id::Numbered));
        }
        where_clause
            .predicates
            .push(parse_quote!(#numbers: ::nitori_driver::id::MixParams));
        where_clause.predicates.push(parse_quote!(
            #bits: ::nitori_driver::id::Xor<<#numbers as ::nitori_driver::id::MixParams>::Out>
        ));
        quote!(::nitori_driver::id::Mixed<#bits, #numbers>)
    };
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let (at_impl_generics, at_ty_generics, at_where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::nitori_driver::id::Numbered for #name #ty_generics #where_clause {
            type Number = #number;
        }
        impl #at_impl_generics ::nitori_driver::need::AtTarget
            for #name #at_ty_generics #at_where_clause
        {
            type Count = ::nitori_driver::need::ExactlyOne;
        }
    })
}

/// What the number of the type named `name` hashes: the crate's name, the
/// type's name and the place of its name, then the place of each macro
/// expansion it came from, the innermost first.
fn place(crate_name: &str, name: &syn::Ident) -> String {
    let mut place = format!("{crate_name}::{name}");
    let mut span = Some(name.span().unwrap());
    while let Some(s) = span {
        // Writing to a `String` does not fail.
        let _ = write!(place, "@{}:{}:{}", s.file(), s.line(), s.column());
        span = s.parent();
    }
    place
}

/// FNV-1a, 64 bits.
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The final mix of splitmix64.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The number as a list of bits, the highest first.
fn bits(h: u64) -> proc_macro2::TokenStream {
    let mut out = quote!(::nitori_driver::id::BNil);
    for i in 0..64 {
        let bit = if (h >> i) & 1 == 1 {
            quote!(::nitori_driver::id::B1)
        } else {
            quote!(::nitori_driver::id::B0)
        };
        out = quote!(::nitori_driver::id::BCons<#bit, #out>);
    }
    out
}

/// Refuse a number given to another type of this crate. The table lives
/// as long as the compiler process, which compiles one crate.
fn check_unique(n: u64, place: &str, name: &syn::Ident) -> syn::Result<()> {
    static SEEN: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match seen.get(&n) {
        Some(other) if other != place => Err(syn::Error::new_spanned(
            name,
            format!(
                "`{name}` has the number of another type of this crate ({other}); rename one of them"
            ),
        )),
        Some(_) => Ok(()),
        None => {
            seen.insert(n, place.to_owned());
            Ok(())
        }
    }
}
