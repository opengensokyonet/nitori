//! Experimental syntax compiler; rustc still checks all value types and lifetimes.
#![forbid(unsafe_code)]
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, parse_quote,
    visit_mut::{self, VisitMut},
    *,
};

fn camel(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut letters = part.chars();
            letters.next().unwrap().to_uppercase().collect::<String>() + letters.as_str()
        })
        .collect()
}

mod current;
/// Define a named operation using the temporary resume environment.
#[proc_macro_attribute]
pub fn call(attribute: TokenStream, item: TokenStream) -> TokenStream {
    current::expand_function(attribute.into(), parse_macro_input!(item as ItemFn))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
/// Define an inline operation using the temporary resume environment.
#[proc_macro]
pub fn call_closure(input: TokenStream) -> TokenStream {
    current::expand(parse_macro_input!(input as ExprClosure))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
