//! The `server_name` extension (RFC 6066 §3).
//!
//! In a ClientHello its data is a list of names; host_name is the only name
//! type, and a list may hold at most one name of each type, so the data is
//! one [`ServerName`]. In EncryptedExtensions the data is empty.

use crate::{vector::LengthOutOfRangeError, width::Width};
use snafu::Snafu;

/// The data of a `server_name` extension in a ClientHello: one host_name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerName<C> {
    host_name: C,
}

/// The name type of host_name.
const HOST_NAME: u8 = 0;

impl<C: AsRef<[u8]>> ServerName<C> {
    /// The list that holds `host_name`, the bytes of a DNS host name.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `host_name` is empty or longer than 65530
    /// bytes, so that the list fits the extension.
    pub fn new(host_name: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(host_name.as_ref().len(), 1..=Width::U16.max() - 5)?;
        Ok(Self { host_name })
    }

    /// The number of bytes [`codec::write_server_name`] writes.
    pub fn encoded_len(&self) -> usize {
        5 + self.host_name.as_ref().len()
    }
}

impl<C> ServerName<C> {
    /// The host name.
    pub fn host_name(&self) -> &C {
        &self.host_name
    }
}

/// A name of a type other than host_name, whose layout is not known, so the
/// list cannot be read further.
///
/// It is constructed only for another type, so the type is never 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("server name of unknown type {name_type}"))]
pub struct UnknownNameTypeError {
    name_type: u8,
}

impl UnknownNameTypeError {
    /// The name type.
    pub const fn name_type(self) -> u8 {
        self.name_type
    }
}

pub mod codec {
    //! Reading and writing `server_name` data.

    use super::{HOST_NAME, ServerName, UnknownNameTypeError};
    use crate::{
        alert::AlertDescription,
        source::read_u8,
        vector::{ReadBoundedVectorError, read_bounded_vector, write_length, write_vector},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads the data of a `server_name` extension in a ClientHello.
    ///
    /// # Errors
    ///
    /// [`ReadServerNameError`].
    pub fn read_server_name<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ServerName<S>, ReadServerNameError> {
        use read_server_name_error::*;
        let range = 1..=Width::U16.max();
        let mut list = read_bounded_vector(src, Width::U16, range.clone()).context(ListSnafu)?;
        let mut host_name = None;
        while list.has_remaining() {
            let name = read_server_name_entry(&mut list)?;
            if host_name.replace(name).is_some() {
                return DuplicateHostNameSnafu.fail();
            }
        }
        // invariant: the list is not empty, and every name in it is a
        // host_name.
        let host_name = host_name.expect("a nonempty list holds a host name");
        Ok(ServerName { host_name })
    }

    /// Reads one name of a server name list, which must be a host_name.
    fn read_server_name_entry<S: Buf + SplitTo>(src: &mut S) -> Result<S, ReadServerNameError> {
        use read_server_name_error::*;
        let name_type = read_u8(src).context(NameTypeSnafu)?;
        if name_type != HOST_NAME {
            return Err(UnknownNameTypeError { name_type }.into());
        }
        read_bounded_vector(src, Width::U16, 1..=Width::U16.max()).context(HostNameSnafu)
    }

    /// Failures that [`read_server_name`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_server_name_error), visibility(pub(crate)))]
    pub enum ReadServerNameError {
        /// The list could not be read.
        #[snafu(display("server name list read failed"))]
        List {
            /// The failure of reading the list.
            source: ReadBoundedVectorError,
        },
        /// The name type of an entry could not be read.
        #[snafu(display("server name type read failed"))]
        NameType {
            /// The shortfall.
            source: IncompleteError,
        },
        /// An entry has a name type other than host_name.
        #[snafu(transparent)]
        UnknownNameType {
            /// The type.
            source: UnknownNameTypeError,
        },
        /// A host name could not be read.
        #[snafu(display("host name read failed"))]
        HostName {
            /// The failure of reading the name.
            source: ReadBoundedVectorError,
        },
        /// The list holds more than one host_name (RFC 6066 §3).
        #[snafu(display("server name list holds more than one host name"))]
        DuplicateHostName,
    }

    impl ReadServerNameError {
        /// The alert the failure calls for: illegal_parameter for a second
        /// host name, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::DuplicateHostName => AlertDescription::ILLEGAL_PARAMETER,
                _ => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes the data of a `server_name` extension in a ClientHello.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_server_name<C: AsRef<[u8]>>(dst: &mut impl BufMut, name: &ServerName<C>) {
        name.encode(dst)
    }

    impl<C: AsRef<[u8]>> ServerName<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            let host_name = self.host_name.as_ref();
            write_length(dst, Width::U16, 3 + host_name.len());
            dst.put_u8(HOST_NAME);
            write_vector(dst, Width::U16, host_name)
        }
    }
}
