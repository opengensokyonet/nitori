#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

//! Current coroutine-based DATA-frame codec example.

pub mod current_codec;
#[cfg(test)]
mod helper_tests;
#[cfg(test)]
mod stack_tests;
