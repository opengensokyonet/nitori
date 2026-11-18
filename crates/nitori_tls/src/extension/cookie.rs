//! The `cookie` extension (RFC 9846 §4.3.2).

use crate::{vector::LengthOutOfRangeError, width::Width};

/// The data of a `cookie` extension: an opaque value of 1 to 65533 bytes, so
/// that the extension data fits its two-byte length.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cookie<C> {
    cookie: C,
}

impl<C: AsRef<[u8]>> Cookie<C> {
    /// The cookie `cookie`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `cookie` is empty or longer than 65533 bytes.
    pub fn new(cookie: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(cookie.as_ref().len(), 1..=Width::U16.max() - 2)?;
        Ok(Self { cookie })
    }

    /// The number of bytes [`codec::write_cookie`] writes.
    pub fn encoded_len(&self) -> usize {
        2 + self.cookie.as_ref().len()
    }
}

impl<C> Cookie<C> {
    /// The cookie.
    pub fn cookie(&self) -> &C {
        &self.cookie
    }
}

pub mod codec {
    //! Reading and writing cookies.

    use super::Cookie;
    use crate::{
        vector::{ReadBoundedVectorError, read_bounded_vector, write_vector},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::SplitTo;

    /// Reads the data of a `cookie` extension.
    ///
    /// # Errors
    ///
    /// [`ReadBoundedVectorError`], for a cookie that is empty or not all
    /// there.
    pub fn read_cookie<S: Buf + SplitTo>(src: &mut S) -> Result<Cookie<S>, ReadBoundedVectorError> {
        let cookie = read_bounded_vector(src, Width::U16, 1..=Width::U16.max())?;
        Ok(Cookie { cookie })
    }

    /// Writes the data of a `cookie` extension.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_cookie<C: AsRef<[u8]>>(dst: &mut impl BufMut, cookie: &Cookie<C>) {
        cookie.encode(dst)
    }

    impl<C: AsRef<[u8]>> Cookie<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            write_vector(dst, Width::U16, self.cookie.as_ref())
        }
    }
}
