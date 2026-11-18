//! NewSessionTicket (RFC 9846 §4.7.1).

use crate::extension::extension_subset;
use crate::{
    extension::{
        BlockEntry, ExtensionType, InvalidExtensionBlockError, UnknownExtension, check_block,
        early_data::MaxEarlyDataSize,
    },
    vector::LengthOutOfRangeError,
    width::Width,
};
use snafu::{ResultExt, Snafu};

/// The lengths an extension block of a NewSessionTicket may have.
const BLOCK: core::ops::RangeInclusive<usize> = 0..=Width::U16.max() - 1;

/// How long a ticket may be used, in seconds: at most 604800, seven days
/// (RFC 9846 §4.7.1). Zero asks the client to discard the ticket at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TicketLifetime(u32);

impl TicketLifetime {
    /// The longest lifetime, seven days.
    pub const MAX: Self = Self(604_800);

    /// A lifetime of `seconds`.
    ///
    /// # Errors
    ///
    /// [`TicketLifetimeTooLongError`] for more than seven days.
    pub const fn new(seconds: u32) -> Result<Self, TicketLifetimeTooLongError> {
        if seconds > Self::MAX.0 {
            return Err(TicketLifetimeTooLongError { seconds });
        }
        Ok(Self(seconds))
    }

    /// The lifetime in seconds.
    pub const fn seconds(self) -> u32 {
        self.0
    }
}

/// A ticket lifetime above seven days.
///
/// It is constructed only when the check fails, so the lifetime is always
/// above 604800 seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("ticket lifetime of {seconds} seconds exceeds 604800"))]
pub struct TicketLifetimeTooLongError {
    seconds: u32,
}

impl TicketLifetimeTooLongError {
    /// The rejected lifetime.
    pub const fn seconds(self) -> u32 {
        self.seconds
    }
}

/// The nonce of a ticket: 0 to 255 bytes, from which the ticket's PSK is
/// derived (RFC 9846 §4.7.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TicketNonce<C> {
    nonce: C,
}

impl<C: AsRef<[u8]>> TicketNonce<C> {
    /// The nonce `nonce`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] for a nonce above 255 bytes.
    pub fn new(nonce: C) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(nonce.as_ref().len(), 0..=Width::U8.max())?;
        Ok(Self { nonce })
    }
}

impl<C> TicketNonce<C> {
    /// The nonce.
    pub fn nonce(&self) -> &C {
        &self.nonce
    }
}

/// A NewSessionTicket.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NewSessionTicket<C> {
    ticket_lifetime: TicketLifetime,
    ticket_age_add: u32,
    ticket_nonce: TicketNonce<C>,
    ticket: C,
    extensions: Vec<NewSessionTicketExtension<C>>,
}

/// An extension of a NewSessionTicket.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NewSessionTicketExtension<C> {
    /// `early_data`: the ticket allows 0-RTT.
    EarlyData(MaxEarlyDataSize),
    /// An extension the readers do not recognize; a server may send it
    /// unsolicited (RFC 9846 §4.3).
    Unknown(UnknownExtension<C>),
}

impl<C: AsRef<[u8]>> BlockEntry for NewSessionTicketExtension<C> {
    fn extension_type(&self) -> ExtensionType {
        match self {
            Self::EarlyData(_) => ExtensionType::EARLY_DATA,
            Self::Unknown(extension) => extension.extension_type(),
        }
    }

    fn data_len(&self) -> usize {
        match self {
            Self::EarlyData(_) => 4,
            Self::Unknown(extension) => extension.data().as_ref().len(),
        }
    }
}

impl<C: AsRef<[u8]>> NewSessionTicket<C> {
    /// The message with these fields.
    ///
    /// # Errors
    ///
    /// [`InvalidNewSessionTicketError::Ticket`] for an empty ticket or one above
    /// 65535 bytes, and [`InvalidNewSessionTicketError::Extensions`] for an
    /// extension type that appears twice or
    /// a block above 65534 bytes.
    pub fn new(
        ticket_lifetime: TicketLifetime,
        ticket_age_add: u32,
        ticket_nonce: TicketNonce<C>,
        ticket: C,
        extensions: Vec<NewSessionTicketExtension<C>>,
    ) -> Result<Self, InvalidNewSessionTicketError> {
        use invalid_new_session_ticket::*;
        LengthOutOfRangeError::check(ticket.as_ref().len(), 1..=Width::U16.max())
            .context(TicketSnafu)?;
        check_block(&extensions, BLOCK).context(ExtensionsSnafu)?;
        Ok(Self {
            ticket_lifetime,
            ticket_age_add,
            ticket_nonce,
            ticket,
            extensions,
        })
    }

    fn block_len(&self) -> usize {
        self.extensions
            .iter()
            .map(|extension| 4 + extension.data_len())
            .sum()
    }

    fn body_len(&self) -> usize {
        4 + 4
            + 1
            + self.ticket_nonce.nonce.as_ref().len()
            + 2
            + self.ticket.as_ref().len()
            + 2
            + self.block_len()
    }

    /// The number of bytes [`codec::write_new_session_ticket`] writes, the
    /// message header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.body_len()
    }
}

impl<C> NewSessionTicket<C> {
    /// The ticket_lifetime field.
    pub fn ticket_lifetime(&self) -> TicketLifetime {
        self.ticket_lifetime
    }

    /// The ticket_age_add field.
    pub fn ticket_age_add(&self) -> u32 {
        self.ticket_age_add
    }

    /// The ticket_nonce field.
    pub fn ticket_nonce(&self) -> &TicketNonce<C> {
        &self.ticket_nonce
    }

    /// The ticket.
    pub fn ticket(&self) -> &C {
        &self.ticket
    }

    /// The extensions, in the order of the wire.
    pub fn extensions(&self) -> &[NewSessionTicketExtension<C>] {
        &self.extensions
    }
}

/// Failures of [`NewSessionTicket::new`].
#[derive(Debug, Snafu)]
#[snafu(module(invalid_new_session_ticket), visibility(pub(crate)))]
pub enum InvalidNewSessionTicketError {
    /// The ticket is empty or too long.
    #[snafu(display("ticket length out of range"))]
    Ticket {
        /// The broken range.
        source: LengthOutOfRangeError,
    },
    /// The extension block is invalid.
    #[snafu(display("NewSessionTicket extensions invalid"))]
    Extensions {
        /// Why.
        source: InvalidExtensionBlockError,
    },
}

extension_subset! {
    /// A recognized extension that a NewSessionTicket may not carry: every one but `early_data`.
    NewSessionTicketDisallowedExtension {
        ServerName,
        SupportedGroups,
        SignatureAlgorithms,
        ApplicationLayerProtocolNegotiation,
        SignedCertificateTimestamp,
        ClientCertificateType,
        ServerCertificateType,
        PreSharedKey,
        SupportedVersions,
        Cookie,
        PskKeyExchangeModes,
        SignatureAlgorithmsCert,
        KeyShare,
        QuicTransportParameters,
        EncryptedClientHello,
    }
}

pub mod codec {
    //! Reading and writing NewSessionTicket messages.

    use super::{
        BLOCK, NewSessionTicket, NewSessionTicketDisallowedExtension, NewSessionTicketExtension,
        TicketLifetime, TicketLifetimeTooLongError, TicketNonce,
    };
    use crate::{
        alert::AlertDescription,
        extension::{
            BlockEntry as _, BlockReader, ExtensionNotAllowedError, ReadExtensionBlockError,
            RestrictedExtension, UnknownExtension, early_data::MaxEarlyDataSize, not_allowed,
            read_extension_block,
        },
        message::{HandshakeType, ReadHeaderError, read_body},
        source::read_u32,
        vector::{
            ExcessBytesError, LengthOutOfRangeError, ReadBoundedVectorError, ReadVectorError,
            read_bounded_vector, read_vector, write_header, write_length, write_vector,
        },
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::{SplitTo, error::IncompleteError};
    use snafu::{ResultExt, Snafu};

    /// Reads a NewSessionTicket message, header included.
    ///
    /// # Errors
    ///
    /// [`ReadNewSessionTicketError`].
    pub fn read_new_session_ticket<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<NewSessionTicket<S>, ReadNewSessionTicketError> {
        let mut body = read_body(src, HandshakeType::NEW_SESSION_TICKET)?;
        let ticket = read_new_session_ticket_body(&mut body)?;
        ExcessBytesError::check(body.remaining())
            .context(read_new_session_ticket_error::ExcessSnafu)?;
        Ok(ticket)
    }

    fn read_new_session_ticket_body<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<NewSessionTicket<S>, ReadNewSessionTicketError> {
        use read_new_session_ticket_error::*;
        let ticket_lifetime = TicketLifetime::new(read_u32(src).context(LifetimeSnafu)?)?;
        let ticket_age_add = read_u32(src).context(AgeAddSnafu)?;
        let nonce = read_vector(src, Width::U8).context(NonceSnafu)?;
        let ticket_nonce = TicketNonce { nonce };
        let ticket =
            read_bounded_vector(src, Width::U16, 1..=Width::U16.max()).context(TicketSnafu)?;
        let block = read_extension_block(src).context(ExtensionsSnafu)?;
        LengthOutOfRangeError::check(block.remaining(), BLOCK).context(ExtensionsLengthSnafu)?;
        let mut block = BlockReader::new(block);
        let mut extensions = Vec::new();
        while let Some((extension_type, mut data)) = block.next().context(ExtensionsSnafu)? {
            let Some(restricted) = extension_type.restricted() else {
                extensions.push(NewSessionTicketExtension::Unknown(UnknownExtension::read(
                    extension_type,
                    data,
                )));
                continue;
            };
            extensions.push(match restricted {
                RestrictedExtension::EarlyData => NewSessionTicketExtension::EarlyData(
                    MaxEarlyDataSize::new(read_u32(&mut data).context(EarlyDataSnafu)?),
                ),
                other => {
                    return Err(not_allowed(other, NewSessionTicketDisallowedExtension::of).into());
                }
            });
            // early_data is the one recognized extension a NewSessionTicket
            // may carry, and the only one read here.
            ExcessBytesError::check(data.remaining()).context(EarlyDataExcessSnafu)?;
        }
        Ok(NewSessionTicket {
            ticket_lifetime,
            ticket_age_add,
            ticket_nonce,
            ticket,
            extensions,
        })
    }

    /// Failures that [`read_new_session_ticket`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(module(read_new_session_ticket_error), visibility(pub(crate)))]
    pub enum ReadNewSessionTicketError {
        /// The header could not be read, or the message is of another type.
        #[snafu(transparent)]
        Header {
            /// The failure.
            source: ReadHeaderError,
        },
        /// The body is longer than its fields.
        #[snafu(display("NewSessionTicket body has excess bytes"))]
        Excess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// ticket_lifetime could not be read.
        #[snafu(display("ticket_lifetime read failed"))]
        Lifetime {
            /// The shortfall.
            source: IncompleteError,
        },
        /// ticket_lifetime is above seven days.
        #[snafu(transparent)]
        LifetimeTooLong {
            /// The lifetime.
            source: TicketLifetimeTooLongError,
        },
        /// ticket_age_add could not be read.
        #[snafu(display("ticket_age_add read failed"))]
        AgeAdd {
            /// The shortfall.
            source: IncompleteError,
        },
        /// ticket_nonce could not be read.
        #[snafu(display("ticket_nonce read failed"))]
        Nonce {
            /// The failure.
            source: ReadVectorError,
        },
        /// ticket could not be read, or is empty.
        #[snafu(display("ticket read failed"))]
        Ticket {
            /// The failure.
            source: ReadBoundedVectorError,
        },
        /// The extension block could not be read, or a type appears twice.
        #[snafu(display("NewSessionTicket extensions read failed"))]
        Extensions {
            /// The failure.
            source: ReadExtensionBlockError,
        },
        /// The extension block is 65535 bytes long.
        #[snafu(display("NewSessionTicket extension block length out of range"))]
        ExtensionsLength {
            /// The broken range.
            source: LengthOutOfRangeError,
        },
        /// A recognized extension that this message may not carry.
        #[snafu(transparent)]
        ExtensionNotAllowed {
            /// The extension.
            source: ExtensionNotAllowedError<NewSessionTicketDisallowedExtension>,
        },
        /// The data of `early_data`, the one recognized extension this
        /// message may carry, is longer than its four bytes.
        #[snafu(display("early_data data has excess bytes"))]
        EarlyDataExcess {
            /// The excess.
            source: ExcessBytesError,
        },
        /// `early_data` could not be read.
        #[snafu(display("early_data max_early_data_size read failed"))]
        EarlyData {
            /// The shortfall.
            source: IncompleteError,
        },
    }

    impl ReadNewSessionTicketError {
        /// The alert the failure calls for: illegal_parameter for a lifetime
        /// above seven days, a type that appears twice and a recognized
        /// extension this message may not carry, decode_error for every
        /// failure to decode.
        pub fn alert(&self) -> AlertDescription {
            match self {
                Self::Header { source } => source.alert(),
                Self::LifetimeTooLong { .. } => AlertDescription::ILLEGAL_PARAMETER,
                Self::Extensions { source } => source.alert(),
                Self::ExtensionNotAllowed { source } => source.alert(),
                Self::Excess { .. }
                | Self::Lifetime { .. }
                | Self::AgeAdd { .. }
                | Self::Nonce { .. }
                | Self::Ticket { .. }
                | Self::ExtensionsLength { .. }
                | Self::EarlyDataExcess { .. }
                | Self::EarlyData { .. } => AlertDescription::DECODE_ERROR,
            }
        }
    }

    /// Writes `ticket`, header included.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_new_session_ticket<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        ticket: &NewSessionTicket<C>,
    ) {
        write_header(dst, HandshakeType::NEW_SESSION_TICKET, ticket.body_len());
        dst.put_u32(ticket.ticket_lifetime.seconds());
        dst.put_u32(ticket.ticket_age_add);
        write_vector(dst, Width::U8, ticket.ticket_nonce.nonce.as_ref());
        write_vector(dst, Width::U16, ticket.ticket.as_ref());
        write_length(dst, Width::U16, ticket.block_len());
        for extension in &ticket.extensions {
            dst.put_u16(extension.extension_type().get());
            write_length(dst, Width::U16, extension.data_len());
            match extension {
                NewSessionTicketExtension::EarlyData(size) => dst.put_u32(size.get()),
                NewSessionTicketExtension::Unknown(extension) => {
                    dst.put_slice(extension.data().as_ref())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NewSessionTicketExtension, TicketLifetime,
        codec::{ReadNewSessionTicketError, read_new_session_ticket},
    };
    use crate::{
        alert::AlertDescription,
        extension::early_data::MaxEarlyDataSize,
        test_support::{concat, extension, message, vector},
    };

    fn ticket(lifetime: u32, ticket: &[u8], extensions: &[u8]) -> Vec<u8> {
        message(
            4,
            &concat(&[
                &lifetime.to_be_bytes(),
                &[0, 0, 0, 1],
                &vector(1, &[0]),
                &vector(2, ticket),
                &vector(2, extensions),
            ]),
        )
    }

    #[test]
    fn ticket_rules() {
        let bytes = ticket(604_800, &[1], &extension(42, &[0xff, 0xff, 0xff, 0xff]));
        let read = read_new_session_ticket(&mut bytes.as_slice()).unwrap();
        assert_eq!(read.ticket_lifetime(), TicketLifetime::MAX);
        assert!(matches!(
            read.extensions(),
            [NewSessionTicketExtension::EarlyData(MaxEarlyDataSize::QUIC)]
        ));
        let error =
            read_new_session_ticket(&mut ticket(604_801, &[1], &[]).as_slice()).unwrap_err();
        let ReadNewSessionTicketError::LifetimeTooLong { source } = &error else {
            panic!("{error:?}")
        };
        assert_eq!(source.seconds(), 604_801);
        assert_eq!(error.alert(), AlertDescription::ILLEGAL_PARAMETER);
        let error = read_new_session_ticket(&mut ticket(1, &[], &[]).as_slice()).unwrap_err();
        assert!(matches!(error, ReadNewSessionTicketError::Ticket { .. }));
        let error =
            read_new_session_ticket(&mut ticket(1, &[1], &extension(42, &[0xff])).as_slice())
                .unwrap_err();
        assert!(matches!(error, ReadNewSessionTicketError::EarlyData { .. }));
        let error =
            read_new_session_ticket(&mut ticket(1, &[1], &extension(42, &[0xff; 5])).as_slice())
                .unwrap_err();
        assert!(matches!(
            error,
            ReadNewSessionTicketError::EarlyDataExcess { .. }
        ));
        let error =
            read_new_session_ticket(&mut ticket(1, &[1], &extension(41, &[0, 0])).as_slice())
                .unwrap_err();
        assert!(matches!(
            error,
            ReadNewSessionTicketError::ExtensionNotAllowed { .. }
        ));
    }
}
