//! The `application_layer_protocol_negotiation` extension (RFC 7301 §3.1).
//!
//! In a ClientHello its data is the [`ProtocolNameList`] the client offers;
//! in EncryptedExtensions it is a list of exactly one name, the
//! [`ProtocolName`] the server selected.

use crate::{vector::LengthOutOfRangeError, width::Width};
use snafu::Snafu;

/// An application protocol name: 1 to 255 bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtocolName<C> {
    name: C,
}

impl<C: AsRef<[u8]>> ProtocolName<C> {
    /// The protocol name `name`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `name` is empty or longer than 255 bytes.
    pub fn new(name: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(name.as_ref().len(), 1..=Width::U8.max())?;
        Ok(Self { name })
    }

    /// The number of bytes the name takes in a list.
    fn entry_len(&self) -> usize {
        1 + self.name.as_ref().len()
    }

    /// The number of bytes [`codec::write_selected_protocol`] writes.
    pub fn encoded_len(&self) -> usize {
        2 + self.entry_len()
    }
}

impl<C> ProtocolName<C> {
    /// The name.
    pub fn name(&self) -> &C {
        &self.name
    }
}

/// A nonempty list of protocol names, in preference order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtocolNameList<C> {
    names: Vec<ProtocolName<C>>,
}

impl<C: AsRef<[u8]>> ProtocolNameList<C> {
    /// The list of `names`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `names` is empty or its encoding is longer
    /// than 65533 bytes, so that the list fits the extension.
    pub fn new(names: Vec<ProtocolName<C>>) -> Result<Self, LengthOutOfRangeError> {
        let len = names.iter().map(ProtocolName::entry_len).sum();
        LengthOutOfRangeError::check(len, 2..=Width::U16.max() - 2)?;
        Ok(Self { names })
    }

    /// The number of bytes [`codec::write_protocol_name_list`] writes.
    pub fn encoded_len(&self) -> usize {
        2 + self
            .names
            .iter()
            .map(ProtocolName::entry_len)
            .sum::<usize>()
    }
}

impl<C> ProtocolNameList<C> {
    /// The names.
    pub fn names(&self) -> &[ProtocolName<C>] {
        &self.names
    }
}

/// A list that should hold the one selected protocol holds more.
///
/// It is constructed only when the check fails, so the count is at least 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("selected protocol list holds {count} names instead of one"))]
pub struct ProtocolsNotSingleError {
    count: usize,
}

impl ProtocolsNotSingleError {
    /// The number of names in the list.
    pub const fn count(self) -> usize {
        self.count
    }
}

pub mod codec {
    //! Reading and writing protocol name lists.

    use super::{ProtocolName, ProtocolNameList, ProtocolsNotSingleError};
    use crate::{
        alert::AlertDescription,
        vector::{ReadBoundedVectorError, read_bounded_vector, write_length, write_vector},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::SplitTo;
    use snafu::{ResultExt, Snafu};

    /// Reads a protocol name list.
    ///
    /// # Errors
    ///
    /// [`ReadProtocolNameListError`].
    pub fn read_protocol_name_list<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ProtocolNameList<S>, ReadProtocolNameListError> {
        use read_protocol_name_list_error::*;
        let mut list =
            read_bounded_vector(src, Width::U16, 2..=Width::U16.max()).context(ListSnafu)?;
        let mut names = Vec::new();
        while list.has_remaining() {
            names.push(read_protocol_name(&mut list)?);
        }
        Ok(ProtocolNameList { names })
    }

    fn read_protocol_name<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ProtocolName<S>, ReadProtocolNameListError> {
        let name = read_bounded_vector(src, Width::U8, 1..=Width::U8.max())
            .context(read_protocol_name_list_error::NameSnafu)?;
        Ok(ProtocolName { name })
    }

    /// Failures that [`read_protocol_name_list`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_protocol_name_list_error), visibility(pub(crate)))]
    pub enum ReadProtocolNameListError {
        /// The list could not be read.
        #[snafu(display("protocol name list read failed"))]
        List {
            /// The failure of reading the list.
            source: ReadBoundedVectorError,
        },
        /// A name could not be read.
        #[snafu(display("protocol name read failed"))]
        Name {
            /// The failure of reading the name.
            source: ReadBoundedVectorError,
        },
    }

    impl ReadProtocolNameListError {
        /// The alert the failure calls for: always decode_error.
        pub fn alert(&self) -> AlertDescription {
            AlertDescription::DECODE_ERROR
        }
    }

    /// Writes a protocol name list.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_protocol_name_list<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        list: &ProtocolNameList<C>,
    ) {
        list.encode(dst)
    }

    /// Reads a protocol name list of exactly one name, as EncryptedExtensions
    /// carries the selected protocol.
    ///
    /// # Errors
    ///
    /// [`ReadSelectedProtocolError`].
    pub fn read_selected_protocol<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ProtocolName<S>, ReadSelectedProtocolError> {
        let mut list = read_protocol_name_list(src)?;
        if list.names.len() != 1 {
            let count = list.names.len();
            return Err(ProtocolsNotSingleError { count }.into());
        }
        // invariant: the list holds exactly one name.
        Ok(list.names.pop().expect("one name"))
    }

    /// Failures that [`read_selected_protocol`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_selected_protocol_error), visibility(pub(crate)))]
    pub enum ReadSelectedProtocolError {
        /// The list could not be read.
        #[snafu(transparent)]
        List {
            /// The failure of reading the list.
            source: ReadProtocolNameListError,
        },
        /// The list holds more than one name (RFC 7301 §3.1).
        #[snafu(transparent)]
        NotSingle {
            /// The number of names.
            source: ProtocolsNotSingleError,
        },
    }

    impl ReadSelectedProtocolError {
        /// The alert the failure calls for: illegal_parameter for more than
        /// one name, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::List { source } => source.alert(),
                Self::NotSingle { .. } => AlertDescription::ILLEGAL_PARAMETER,
            }
        }
    }

    /// Writes a protocol name list of the one name `protocol`.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_selected_protocol<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        protocol: &ProtocolName<C>,
    ) {
        protocol.encode_selected(dst)
    }

    impl<C: AsRef<[u8]>> ProtocolName<C> {
        pub(crate) fn encode_selected(&self, dst: &mut impl BufMut) {
            write_length(dst, Width::U16, self.entry_len());
            write_vector(dst, Width::U8, self.name.as_ref())
        }
    }

    impl<C: AsRef<[u8]>> ProtocolNameList<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            write_length(dst, Width::U16, self.encoded_len() - 2);
            for name in &self.names {
                write_vector(dst, Width::U8, name.name.as_ref());
            }
        }
    }
}
