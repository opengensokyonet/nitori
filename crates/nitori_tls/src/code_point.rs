//! Code points of the TLS registries as newtypes, and lists of them.
//!
//! A code point type, such as [`CipherSuite`](crate::cipher_suite::CipherSuite)
//! or [`ExtensionType`](crate::extension::ExtensionType), holds any value of
//! its field: values this crate does not name are code points too, and the
//! readers keep them. `Debug` prints the registry name of a named value.
//!
//! A [`CodePointList`] is a nonempty list of two-byte code points, such as the
//! cipher suites of a ClientHello or the groups of `supported_groups`; a
//! [`ByteCodePointList`] is a nonempty list of one-byte code points, such as
//! the modes of `psk_key_exchange_modes`. The [`codec`] module reads and
//! writes both.

use crate::{vector::LengthOutOfRangeError, width::Width};
use nitori_crypto::{NamedGroup, SignatureScheme};
use snafu::Snafu;

/// Defines a code point newtype with named constants and a `Debug` that
/// prints their names.
macro_rules! code_point {
    (
        $(#[$meta:meta])*
        $name:ident($repr:ty);
        $($(#[$constant_meta:meta])* $constant:ident = $value:literal => $label:literal,)*
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($repr);

        impl $name {
            $($(#[$constant_meta])* pub const $constant: Self = Self($value);)*

            /// The code point with value `value`.
            pub const fn new(value: $repr) -> Self {
                Self(value)
            }

            /// The value.
            pub const fn get(self) -> $repr {
                self.0
            }

            /// The registry name of a value this crate names.
            pub const fn name(self) -> Option<&'static str> {
                match self.0 {
                    $($value => Some($label),)*
                    _ => None,
                }
            }
        }

        /// Prints the registry name and the value, or the value alone.
        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                match self.name() {
                    Some(name) => write!(f, "{}({name}, {:#x})", stringify!($name), self.0),
                    None => write!(f, "{}({:#x})", stringify!($name), self.0),
                }
            }
        }
    };
}

pub(crate) use code_point;

mod sealed {
    pub trait Sealed {}
}

/// A two-byte code point that appears in a [`CodePointList`].
///
/// The list's length field is two bytes wide, except for the versions of
/// `supported_versions`, whose length field is one byte (RFC 9846 §4.3.1).
/// Implemented by the code point types of this crate and of
/// `nitori_crypto` that appear in lists.
pub trait CodePoint: Copy + sealed::Sealed {
    /// The width of the list's length field.
    #[doc(hidden)]
    const LIST_LENGTH: Width;

    /// The code point with value `value`.
    fn from_value(value: u16) -> Self;

    /// The value.
    fn value(self) -> u16;
}

/// A one-byte code point that appears in a [`ByteCodePointList`].
pub trait ByteCodePoint: Copy + sealed::Sealed {
    /// The code point with value `value`.
    fn from_value(value: u8) -> Self;

    /// The value.
    fn value(self) -> u8;
}

macro_rules! list_code_point {
    ($($name:ty => $width:ident, $new:expr, $get:expr;)*) => {$(
        impl sealed::Sealed for $name {}
        impl CodePoint for $name {
            const LIST_LENGTH: Width = Width::$width;
            fn from_value(value: u16) -> Self {
                $new(value)
            }
            fn value(self) -> u16 {
                $get(self)
            }
        }
    )*};
}

list_code_point! {
    crate::cipher_suite::CipherSuite => U16, crate::cipher_suite::CipherSuite::new, crate::cipher_suite::CipherSuite::get;
    NamedGroup => U16, NamedGroup::new, NamedGroup::get;
    SignatureScheme => U16, SignatureScheme::new, SignatureScheme::get;
    crate::version::ProtocolVersion => U8, crate::version::ProtocolVersion::new, crate::version::ProtocolVersion::get;
}

macro_rules! byte_code_point {
    ($($name:ty;)*) => {$(
        impl sealed::Sealed for $name {}
        impl ByteCodePoint for $name {
            fn from_value(value: u8) -> Self {
                <$name>::new(value)
            }
            fn value(self) -> u8 {
                self.get()
            }
        }
    )*};
}

byte_code_point! {
    crate::extension::psk_key_exchange_modes::PskKeyExchangeMode;
    crate::extension::certificate_type::CertificateType;
}

/// A nonempty list of two-byte code points, in the order of the wire.
///
/// A list built here holds at most 32766 code points with a two-byte
/// length field, so that it fits the data of an extension, and 127 with a
/// one-byte field; a list read from a message holds what its length field
/// counts.
/// Duplicates are kept as they are.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodePointList<T> {
    items: Vec<T>,
}

impl<T: CodePoint> CodePointList<T> {
    /// The list of `items`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`], in bytes, if `items` is empty or too long for
    /// the length field.
    pub fn new(items: Vec<T>) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(2 * items.len(), Self::byte_range())?;
        Ok(Self { items })
    }

    /// The encoded lengths a list can have, without its length field: with
    /// its length field, the list fits the data of an extension.
    fn byte_range() -> core::ops::RangeInclusive<usize> {
        2..=T::LIST_LENGTH.max().min(Width::U16.max() - 2) & !1
    }

    /// The number of bytes [`codec::write_code_point_list`] writes.
    pub fn encoded_len(&self) -> usize {
        T::LIST_LENGTH.len() + 2 * self.items.len()
    }
}

impl<T> CodePointList<T> {
    /// The code points.
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Takes the code points.
    pub fn into_vec(self) -> Vec<T> {
        self.items
    }
}

/// A nonempty list of one-byte code points, of at most 255.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ByteCodePointList<T> {
    items: Vec<T>,
}

impl<T: ByteCodePoint> ByteCodePointList<T> {
    /// The list of `items`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `items` is empty or longer than 255.
    pub fn new(items: Vec<T>) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(items.len(), 1..=Width::U8.max())?;
        Ok(Self { items })
    }

    /// The number of bytes [`codec::write_byte_code_point_list`] writes.
    pub fn encoded_len(&self) -> usize {
        1 + self.items.len()
    }
}

impl<T> ByteCodePointList<T> {
    /// The code points.
    pub fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Takes the code points.
    pub fn into_vec(self) -> Vec<T> {
        self.items
    }
}

/// The length of a list of two-byte code points is odd.
///
/// It is constructed only when the check fails, so the length is always odd.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("list of two-byte code points has an odd length of {len} bytes"))]
pub struct OddLengthError {
    len: usize,
}

impl OddLengthError {
    /// The length, in bytes.
    pub const fn length(self) -> usize {
        self.len
    }

    pub(crate) fn check(len: usize) -> Result<(), Self> {
        if len % 2 == 1 {
            return Err(Self { len });
        }
        Ok(())
    }
}

pub mod codec {
    //! Reading and writing code point lists.

    use super::{ByteCodePoint, ByteCodePointList, CodePoint, CodePointList, OddLengthError};
    use crate::{
        alert::AlertDescription,
        source::read_bytes,
        vector::{read_length, write_length},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads a nonempty list of two-byte code points.
    ///
    /// # Errors
    ///
    /// [`ReadCodePointListError`].
    pub fn read_code_point_list<T: CodePoint, S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<CodePointList<T>, ReadCodePointListError> {
        use read_code_point_list_error::*;
        let len = read_length(src, T::LIST_LENGTH).context(LengthSnafu)?;
        if len == 0 {
            return EmptySnafu.fail();
        }
        OddLengthError::check(len)?;
        let mut items = read_bytes(src, len).context(ItemsSnafu)?;
        let mut list = Vec::with_capacity(len / 2);
        while items.has_remaining() {
            list.push(T::from_value(items.get_u16()));
        }
        Ok(CodePointList { items: list })
    }

    /// Failures that [`read_code_point_list`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_code_point_list_error), visibility(pub(crate)))]
    pub enum ReadCodePointListError {
        /// The length field could not be read.
        #[snafu(display("code point list length read failed"))]
        Length {
            /// The shortfall.
            source: IncompleteError,
        },
        /// The list is empty.
        #[snafu(display("code point list is empty"))]
        Empty,
        /// The length is not a multiple of two.
        #[snafu(transparent)]
        OddLength {
            /// The broken rule.
            source: OddLengthError,
        },
        /// The code points could not be read.
        #[snafu(display("code point list read failed"))]
        Items {
            /// The shortfall.
            source: IncompleteError,
        },
    }

    impl ReadCodePointListError {
        /// The alert the failure calls for: always decode_error.
        pub fn alert(&self) -> AlertDescription {
            AlertDescription::DECODE_ERROR
        }
    }

    /// Writes `list` with its length field.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for the list, as [`BufMut`] does.
    pub fn write_code_point_list<T: CodePoint>(dst: &mut impl BufMut, list: &CodePointList<T>) {
        let items = list.as_slice();
        write_length(dst, T::LIST_LENGTH, 2 * items.len());
        for item in items {
            dst.put_u16(item.value());
        }
    }

    /// Reads a nonempty list of one-byte code points.
    ///
    /// # Errors
    ///
    /// [`ReadByteCodePointListError`].
    pub fn read_byte_code_point_list<T: ByteCodePoint, S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ByteCodePointList<T>, ReadByteCodePointListError> {
        use read_byte_code_point_list_error::*;
        let len = read_length(src, Width::U8).context(LengthSnafu)?;
        if len == 0 {
            return EmptySnafu.fail();
        }
        let mut items = read_bytes(src, len).context(ItemsSnafu)?;
        let mut list = Vec::with_capacity(len);
        while items.has_remaining() {
            list.push(T::from_value(items.get_u8()));
        }
        Ok(ByteCodePointList { items: list })
    }

    /// Failures that [`read_byte_code_point_list`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_byte_code_point_list_error), visibility(pub(crate)))]
    pub enum ReadByteCodePointListError {
        /// The length field could not be read.
        #[snafu(display("code point list length read failed"))]
        Length {
            /// The shortfall.
            source: IncompleteError,
        },
        /// The list is empty.
        #[snafu(display("code point list is empty"))]
        Empty,
        /// The code points could not be read.
        #[snafu(display("code point list read failed"))]
        Items {
            /// The shortfall.
            source: IncompleteError,
        },
    }

    impl ReadByteCodePointListError {
        /// The alert the failure calls for: always decode_error.
        pub fn alert(&self) -> AlertDescription {
            AlertDescription::DECODE_ERROR
        }
    }

    /// Writes `list` with its one-byte length field.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for the list, as [`BufMut`] does.
    pub fn write_byte_code_point_list<T: ByteCodePoint>(
        dst: &mut impl BufMut,
        list: &ByteCodePointList<T>,
    ) {
        let items = list.as_slice();
        write_length(dst, Width::U8, items.len());
        for item in items {
            dst.put_u8(item.value());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodePointList, OddLengthError,
        codec::{ReadCodePointListError, read_code_point_list},
    };
    use crate::cipher_suite::CipherSuite;

    #[test]
    fn lists_must_hold_whole_code_points() {
        let mut input = &[0x00, 0x03, 0x13, 0x01, 0x13][..];
        let error = read_code_point_list::<CipherSuite, _>(&mut input).unwrap_err();
        assert!(matches!(
            error,
            ReadCodePointListError::OddLength { source } if source == OddLengthError { len: 3 }
        ));
        let mut input = &[0x00, 0x00][..];
        let error = read_code_point_list::<CipherSuite, _>(&mut input).unwrap_err();
        assert!(matches!(error, ReadCodePointListError::Empty));
        let mut input = &[0x00, 0x04, 0x13, 0x01][..];
        let error = read_code_point_list::<CipherSuite, _>(&mut input).unwrap_err();
        assert!(matches!(error, ReadCodePointListError::Items { .. }));
    }

    #[test]
    fn construction_checks_the_count() {
        assert!(CodePointList::<CipherSuite>::new(Vec::new()).is_err());
        assert!(CodePointList::new(vec![CipherSuite::TLS_AES_128_GCM_SHA256; 32766]).is_ok());
        let error =
            CodePointList::new(vec![CipherSuite::TLS_AES_128_GCM_SHA256; 32767]).unwrap_err();
        assert_eq!((error.length(), error.max()), (65534, 65532));
    }
}
