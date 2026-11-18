//! The `early_data` extension (RFC 9846 §4.3.10).
//!
//! In ClientHello and EncryptedExtensions its data is empty; in
//! NewSessionTicket it is a [`MaxEarlyDataSize`].

/// The max_early_data_size field of `early_data` in a NewSessionTicket.
///
/// In QUIC the field is always [`MaxEarlyDataSize::QUIC`], and the amount of
/// 0-RTT data is limited by transport parameters instead (RFC 9001 §4.6.1);
/// a reader keeps whatever value it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaxEarlyDataSize(u32);

impl MaxEarlyDataSize {
    /// `0xffffffff`, the value a QUIC server sends to allow 0-RTT.
    pub const QUIC: Self = Self(0xffff_ffff);

    /// The field with value `value`.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The value.
    pub const fn get(self) -> u32 {
        self.0
    }
}
