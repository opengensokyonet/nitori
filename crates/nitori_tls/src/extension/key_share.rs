//! The `key_share` extension (RFC 9846 §4.3.8).
//!
//! In a ClientHello its data is [`ClientShares`], the shares the client
//! offers; in a ServerHello it is the one [`KeyShareEntry`] of the server; in
//! a HelloRetryRequest it is the `NamedGroup` the server selected.

use crate::{vector::LengthOutOfRangeError, width::Width};
use nitori_crypto::NamedGroup;
use snafu::{ResultExt, Snafu};

/// One key share: a group and a public value of that group.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyShareEntry<C> {
    group: NamedGroup,
    key_exchange: C,
}

impl<C: AsRef<[u8]>> KeyShareEntry<C> {
    /// The share `key_exchange` of `group`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if `key_exchange` is empty or longer than 65531
    /// bytes, so that the entry fits the extension.
    pub fn new(group: NamedGroup, key_exchange: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(key_exchange.as_ref().len(), 1..=Width::U16.max() - 4)?;
        Ok(Self {
            group,
            key_exchange,
        })
    }

    /// The number of bytes [`codec::write_key_share_entry`] writes.
    pub fn encoded_len(&self) -> usize {
        4 + self.key_exchange.as_ref().len()
    }
}

impl<C> KeyShareEntry<C> {
    /// The group.
    pub fn group(&self) -> NamedGroup {
        self.group
    }

    /// The public value.
    pub fn key_exchange(&self) -> &C {
        &self.key_exchange
    }
}

/// The shares a client offers, at most one for each group.
///
/// The list may be empty, to ask the server to select a group
/// (RFC 9846 §4.3.8).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClientShares<C> {
    entries: Vec<KeyShareEntry<C>>,
}

impl<C: AsRef<[u8]>> ClientShares<C> {
    /// The list of `entries`.
    ///
    /// # Errors
    ///
    /// [`InvalidClientSharesError::DuplicateGroup`] if two entries share a group,
    /// and [`InvalidClientSharesError::Length`] if the encoding is longer than
    /// 65533 bytes, so that the list fits the extension.
    pub fn new(entries: Vec<KeyShareEntry<C>>) -> Result<Self, InvalidClientSharesError> {
        check_groups(&entries)?;
        let len = entries.iter().map(KeyShareEntry::encoded_len).sum();
        LengthOutOfRangeError::check(len, 0..=Width::U16.max() - 2)
            .context(invalid_client_shares::LengthSnafu)?;
        Ok(Self { entries })
    }

    /// The number of bytes [`codec::write_client_shares`] writes.
    pub fn encoded_len(&self) -> usize {
        2 + self
            .entries
            .iter()
            .map(KeyShareEntry::encoded_len)
            .sum::<usize>()
    }
}

impl<C> ClientShares<C> {
    /// The entries, in the order of the wire.
    pub fn entries(&self) -> &[KeyShareEntry<C>] {
        &self.entries
    }

    /// The entry of `group`, if there is one.
    pub fn get(&self, group: NamedGroup) -> Option<&KeyShareEntry<C>> {
        self.entries.iter().find(|entry| entry.group == group)
    }
}

/// Checks that no two entries share a group.
fn check_groups<C>(entries: &[KeyShareEntry<C>]) -> Result<(), DuplicateGroupError> {
    for (index, entry) in entries.iter().enumerate() {
        if entries[..index]
            .iter()
            .any(|seen| seen.group == entry.group)
        {
            return Err(DuplicateGroupError { group: entry.group });
        }
    }
    Ok(())
}

/// Two shares of a client are of the same group (RFC 9846 §4.3.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("more than one key share of {group:?}"))]
pub struct DuplicateGroupError {
    group: NamedGroup,
}

impl DuplicateGroupError {
    /// The group.
    pub const fn group(self) -> NamedGroup {
        self.group
    }
}

/// Failures of [`ClientShares::new`].
#[derive(Debug, Snafu)]
#[snafu(module(invalid_client_shares), visibility(pub(crate)))]
pub enum InvalidClientSharesError {
    /// Two entries share a group.
    #[snafu(transparent)]
    DuplicateGroup {
        /// The group.
        source: DuplicateGroupError,
    },
    /// The list is too long.
    #[snafu(display("key share list too long"))]
    Length {
        /// The broken range.
        source: LengthOutOfRangeError,
    },
}

pub mod codec {
    //! Reading and writing key shares.

    use super::{ClientShares, DuplicateGroupError, KeyShareEntry, check_groups};
    use crate::{
        alert::AlertDescription,
        source::read_u16,
        vector::{
            ReadBoundedVectorError, ReadVectorError, read_bounded_vector, read_vector,
            write_length, write_vector,
        },
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_crypto::NamedGroup;
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads one key share entry.
    ///
    /// # Errors
    ///
    /// [`ReadKeyShareEntryError`].
    pub fn read_key_share_entry<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<KeyShareEntry<S>, ReadKeyShareEntryError> {
        use read_key_share_entry_error::*;
        let group = NamedGroup::new(read_u16(src).context(GroupSnafu)?);
        let key_exchange =
            read_bounded_vector(src, Width::U16, 1..=Width::U16.max()).context(KeyExchangeSnafu)?;
        Ok(KeyShareEntry {
            group,
            key_exchange,
        })
    }

    /// Failures that [`read_key_share_entry`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_key_share_entry_error), visibility(pub(crate)))]
    pub enum ReadKeyShareEntryError {
        /// The group could not be read.
        #[snafu(display("key share group read failed"))]
        Group {
            /// The shortfall.
            source: IncompleteError,
        },
        /// The public value could not be read.
        #[snafu(display("key share value read failed"))]
        KeyExchange {
            /// The failure of reading the value.
            source: ReadBoundedVectorError,
        },
    }

    impl ReadKeyShareEntryError {
        /// The alert the failure calls for: always decode_error.
        pub fn alert(&self) -> AlertDescription {
            AlertDescription::DECODE_ERROR
        }
    }

    /// Writes one key share entry.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_key_share_entry<C: AsRef<[u8]>>(dst: &mut impl BufMut, entry: &KeyShareEntry<C>) {
        entry.encode(dst)
    }

    /// Reads the shares of a ClientHello.
    ///
    /// # Errors
    ///
    /// [`ReadClientSharesError`].
    pub fn read_client_shares<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<ClientShares<S>, ReadClientSharesError> {
        use read_client_shares_error::*;
        let mut list = read_vector(src, Width::U16).context(ListSnafu)?;
        let mut entries = Vec::new();
        while list.has_remaining() {
            entries.push(read_key_share_entry(&mut list).context(EntrySnafu)?);
        }
        check_groups(&entries)?;
        Ok(ClientShares { entries })
    }

    /// Failures that [`read_client_shares`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_client_shares_error), visibility(pub(crate)))]
    pub enum ReadClientSharesError {
        /// The list could not be read.
        #[snafu(display("client key share list read failed"))]
        List {
            /// The failure of reading the list.
            source: ReadVectorError,
        },
        /// An entry could not be read.
        #[snafu(display("client key share read failed"))]
        Entry {
            /// The failure of reading the entry.
            source: ReadKeyShareEntryError,
        },
        /// Two entries share a group; RFC 9846 §4.3.8 lets a server check
        /// this.
        #[snafu(transparent)]
        DuplicateGroup {
            /// The group.
            source: DuplicateGroupError,
        },
    }

    impl ReadClientSharesError {
        /// The alert the failure calls for: illegal_parameter for a group
        /// shared twice, decode_error otherwise.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::DuplicateGroup { .. } => AlertDescription::ILLEGAL_PARAMETER,
                Self::List { .. } | Self::Entry { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes the shares of a ClientHello.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_client_shares<C: AsRef<[u8]>>(dst: &mut impl BufMut, shares: &ClientShares<C>) {
        shares.encode(dst)
    }

    impl<C: AsRef<[u8]>> KeyShareEntry<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            dst.put_u16(self.group.get());
            write_vector(dst, Width::U16, self.key_exchange.as_ref())
        }
    }

    impl<C: AsRef<[u8]>> ClientShares<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            write_length(dst, Width::U16, self.encoded_len() - 2);
            for entry in &self.entries {
                entry.encode(dst);
            }
        }
    }
}
