//! TLS alert descriptions (RFC 9846 §6).
//!
//! In QUIC a TLS alert closes the connection with the transport error code
//! `0x0100` plus the alert description (RFC 9001 §4.8). The errors of this
//! crate that end a handshake answer which alert they call for through their
//! `alert` method.

use crate::code_point::code_point;

code_point! {
    /// An AlertDescription code point (RFC 9846 §6).
    ///
    /// Any byte is a code point.
    AlertDescription(u8);
    /// close_notify.
    CLOSE_NOTIFY = 0 => "close_notify",
    /// unexpected_message.
    UNEXPECTED_MESSAGE = 10 => "unexpected_message",
    /// bad_record_mac.
    BAD_RECORD_MAC = 20 => "bad_record_mac",
    /// record_overflow.
    RECORD_OVERFLOW = 22 => "record_overflow",
    /// handshake_failure.
    HANDSHAKE_FAILURE = 40 => "handshake_failure",
    /// bad_certificate.
    BAD_CERTIFICATE = 42 => "bad_certificate",
    /// unsupported_certificate.
    UNSUPPORTED_CERTIFICATE = 43 => "unsupported_certificate",
    /// certificate_revoked.
    CERTIFICATE_REVOKED = 44 => "certificate_revoked",
    /// certificate_expired.
    CERTIFICATE_EXPIRED = 45 => "certificate_expired",
    /// certificate_unknown.
    CERTIFICATE_UNKNOWN = 46 => "certificate_unknown",
    /// illegal_parameter.
    ILLEGAL_PARAMETER = 47 => "illegal_parameter",
    /// unknown_ca.
    UNKNOWN_CA = 48 => "unknown_ca",
    /// access_denied.
    ACCESS_DENIED = 49 => "access_denied",
    /// decode_error.
    DECODE_ERROR = 50 => "decode_error",
    /// decrypt_error.
    DECRYPT_ERROR = 51 => "decrypt_error",
    /// protocol_version.
    PROTOCOL_VERSION = 70 => "protocol_version",
    /// insufficient_security.
    INSUFFICIENT_SECURITY = 71 => "insufficient_security",
    /// internal_error.
    INTERNAL_ERROR = 80 => "internal_error",
    /// inappropriate_fallback.
    INAPPROPRIATE_FALLBACK = 86 => "inappropriate_fallback",
    /// user_canceled.
    USER_CANCELED = 90 => "user_canceled",
    /// missing_extension.
    MISSING_EXTENSION = 109 => "missing_extension",
    /// unsupported_extension.
    UNSUPPORTED_EXTENSION = 110 => "unsupported_extension",
    /// unrecognized_name.
    UNRECOGNIZED_NAME = 112 => "unrecognized_name",
    /// bad_certificate_status_response.
    BAD_CERTIFICATE_STATUS_RESPONSE = 113 => "bad_certificate_status_response",
    /// unknown_psk_identity.
    UNKNOWN_PSK_IDENTITY = 115 => "unknown_psk_identity",
    /// certificate_required.
    CERTIFICATE_REQUIRED = 116 => "certificate_required",
    /// no_application_protocol.
    NO_APPLICATION_PROTOCOL = 120 => "no_application_protocol",
    /// ech_required (RFC 9849).
    ECH_REQUIRED = 121 => "ech_required",
}

/// A failure that ends a handshake with an alert, which it names.
///
/// The handshake engines take the failures of their capabilities, such as
/// target selection or certificate verification, as sources, and close with
/// the alert each calls for.
pub trait CallsForAlert {
    /// The alert the failure calls for.
    fn alert(&self) -> AlertDescription;
}

impl CallsForAlert for core::convert::Infallible {
    fn alert(&self) -> AlertDescription {
        match *self {}
    }
}
