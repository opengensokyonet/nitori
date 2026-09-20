//! Typed failures retain progress without requiring host errors to be static.
use core::fmt;
use snafu::Snafu;

#[derive(Debug, Snafu)]
pub enum ReadFailure {
    #[snafu(display(
        "destination capacity {available} is below required {required} after {completed} bytes"
    ))]
    Capacity {
        required: usize,
        available: usize,
        completed: usize,
    },
    #[snafu(display("unexpected EOF after {completed} of {expected} bytes"))]
    UnexpectedEof { expected: usize, completed: usize },
    #[snafu(display("read failed after {completed} bytes"))]
    Read { completed: usize },
}

#[derive(Debug, Snafu)]
pub enum WriteFailure {
    #[snafu(display("write failed after {completed} bytes"))]
    Write { completed: usize },
    #[snafu(display("write made no progress after {completed} bytes"))]
    WriteZero { completed: usize },
}

macro_rules! progress_error {
    ($name:ident, $kind:ident, $source_variant:ident, $($constructor:ident => $variant:ident { $($field:ident),* }),*) => {
        /// Failure details and an optional original host error.
        #[derive(Debug)]
        pub struct $name<E> { failure: $kind, source: Option<E> }
        impl<E> $name<E> {
            pub fn failure(&self) -> &$kind { &self.failure }
            pub fn host_error(&self) -> Option<&E> { self.source.as_ref() }
            pub fn into_parts(self) -> ($kind, Option<E>) { (self.failure, self.source) }
            pub fn completed(&self) -> usize {
                match self.failure { $kind::$source_variant { completed } $(| $kind::$variant { completed, .. })* => completed }
            }
            pub(crate) fn host(source: E, completed: usize) -> Self {
                Self { failure: $kind::$source_variant { completed }, source: Some(source) }
            }
            $(pub(crate) fn $constructor($($field: usize,)* completed: usize) -> Self {
                Self { failure: $kind::$variant { $($field,)* completed }, source: None }
            })*
        }
        impl<E> fmt::Display for $name<E> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.failure.fmt(f) }
        }
        impl<E: std::error::Error + 'static> std::error::Error for $name<E> {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.source.as_ref().map(|source| source as _)
            }
        }
    };
}
progress_error!(ReadError, ReadFailure, Read,
    capacity => Capacity { required, available },
    unexpected_eof => UnexpectedEof { expected }
);
progress_error!(WriteError, WriteFailure, Write, write_zero => WriteZero {});
