//! The `psk_key_exchange_modes` extension (RFC 9846 §4.3.9).
//!
//! Its data is a [`ByteCodePointList`](crate::code_point::ByteCodePointList)
//! of [`PskKeyExchangeMode`]s.

use crate::code_point::code_point;

code_point! {
    /// A PskKeyExchangeMode code point.
    ///
    /// Any byte is a code point.
    PskKeyExchangeMode(u8);
    /// psk_ke: PSK-only key establishment.
    PSK_KE = 0 => "psk_ke",
    /// psk_dhe_ke: PSK with (EC)DHE key establishment.
    PSK_DHE_KE = 1 => "psk_dhe_ke",
}
