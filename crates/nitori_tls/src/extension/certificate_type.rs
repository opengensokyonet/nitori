//! The `client_certificate_type` and `server_certificate_type` extensions
//! (RFC 7250 §3).
//!
//! In a ClientHello their data is a
//! [`ByteCodePointList`](crate::code_point::ByteCodePointList) of the
//! [`CertificateType`]s an endpoint supports; in EncryptedExtensions it is
//! the one type selected.

use crate::code_point::code_point;

code_point! {
    /// A certificate type code point (RFC 7250 §3, RFC 9846 §4.5.1).
    ///
    /// Any byte is a code point.
    CertificateType(u8);
    /// X.509 certificates.
    X509 = 0 => "X509",
    /// A raw public key, a SubjectPublicKeyInfo.
    RAW_PUBLIC_KEY = 2 => "RawPublicKey",
}
