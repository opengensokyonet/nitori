//! TLS protocol versions.

use crate::code_point::code_point;

code_point! {
    /// A ProtocolVersion code point (RFC 9846 §4.2.2, §4.3.1).
    ///
    /// Any two-byte value is a code point. TLS 1.3 negotiates the version in
    /// `supported_versions`; the legacy version fields hold
    /// [`ProtocolVersion::TLS_1_2`].
    ProtocolVersion(u16);
    /// TLS 1.2, `0x0303`.
    TLS_1_2 = 0x0303 => "TLS 1.2",
    /// TLS 1.3, `0x0304`.
    TLS_1_3 = 0x0304 => "TLS 1.3",
}
