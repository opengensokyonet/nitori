use core::{fmt, future::Future};

use smallvec::SmallVec;
use snafu::Snafu;

use crate::{Issuer, Supported};

/// A TLS SignatureScheme code point (RFC 8446 §4.2.3).
///
/// Any 16-bit value is a code point; the constants name the schemes that
/// implementations in this family provide.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignatureScheme(u16);

impl SignatureScheme {
    /// rsa_pkcs1_sha256: RSASSA-PKCS1-v1_5 with SHA-256, for certificates
    /// only (RFC 8446 §4.2.3).
    pub const RSA_PKCS1_SHA256: Self = Self(0x0401);
    /// rsa_pkcs1_sha384: RSASSA-PKCS1-v1_5 with SHA-384, for certificates
    /// only.
    pub const RSA_PKCS1_SHA384: Self = Self(0x0501);
    /// rsa_pkcs1_sha512: RSASSA-PKCS1-v1_5 with SHA-512, for certificates
    /// only.
    pub const RSA_PKCS1_SHA512: Self = Self(0x0601);
    /// ecdsa_secp256r1_sha256: ECDSA over P-256 with SHA-256.
    pub const ECDSA_SECP256R1_SHA256: Self = Self(0x0403);
    /// ecdsa_secp384r1_sha384: ECDSA over P-384 with SHA-384.
    pub const ECDSA_SECP384R1_SHA384: Self = Self(0x0503);
    /// ecdsa_secp521r1_sha512: ECDSA over P-521 with SHA-512.
    pub const ECDSA_SECP521R1_SHA512: Self = Self(0x0603);
    /// rsa_pss_rsae_sha256: RSASSA-PSS with SHA-256 and an rsaEncryption
    /// key.
    pub const RSA_PSS_RSAE_SHA256: Self = Self(0x0804);
    /// rsa_pss_rsae_sha384: RSASSA-PSS with SHA-384 and an rsaEncryption
    /// key.
    pub const RSA_PSS_RSAE_SHA384: Self = Self(0x0805);
    /// rsa_pss_rsae_sha512: RSASSA-PSS with SHA-512 and an rsaEncryption
    /// key.
    pub const RSA_PSS_RSAE_SHA512: Self = Self(0x0806);
    /// ed25519: EdDSA over Curve25519 (RFC 8032).
    pub const ED25519: Self = Self(0x0807);

    /// The scheme with code point `value`.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The code point.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    const fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::RSA_PKCS1_SHA256 => "rsa_pkcs1_sha256",
            Self::RSA_PKCS1_SHA384 => "rsa_pkcs1_sha384",
            Self::RSA_PKCS1_SHA512 => "rsa_pkcs1_sha512",
            Self::ECDSA_SECP256R1_SHA256 => "ecdsa_secp256r1_sha256",
            Self::ECDSA_SECP384R1_SHA384 => "ecdsa_secp384r1_sha384",
            Self::ECDSA_SECP521R1_SHA512 => "ecdsa_secp521r1_sha512",
            Self::RSA_PSS_RSAE_SHA256 => "rsa_pss_rsae_sha256",
            Self::RSA_PSS_RSAE_SHA384 => "rsa_pss_rsae_sha384",
            Self::RSA_PSS_RSAE_SHA512 => "rsa_pss_rsae_sha512",
            Self::ED25519 => "ed25519",
            _ => return None,
        })
    }
}

impl fmt::Debug for SignatureScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "SignatureScheme({name})"),
            None => write!(f, "SignatureScheme({:#06x})", self.0),
        }
    }
}

/// Signature verification, by SignatureScheme.
pub trait SignatureVerify: Issuer {
    /// Every scheme for which [`SignatureVerify::signature_scheme`] issues a
    /// [`Supported`].
    fn signature_schemes(&self) -> &[SignatureScheme];

    /// Whether this implementation verifies signatures of `scheme`.
    fn signature_scheme(&self, scheme: SignatureScheme)
    -> Option<Supported<Self, SignatureScheme>>;

    /// Check that `signature` is a signature of `message` under
    /// `public_key`.
    ///
    /// `public_key` is the content of the `subjectPublicKey` bit string of a
    /// SubjectPublicKeyInfo, without the algorithm identifier: an
    /// uncompressed point for ECDSA, 32 bytes for Ed25519, and a DER
    /// RSAPublicKey for RSA. Whether the key's algorithm matches `scheme` is
    /// for the caller to check from the algorithm identifier.
    ///
    /// # Errors
    ///
    /// Fails when the signature does not verify, including when the public
    /// key is malformed or of a size the scheme does not accept.
    fn verify(
        &self,
        scheme: Supported<Self, SignatureScheme>,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), BadSignature>;
}

/// A signature does not verify.
#[derive(Debug, Snafu)]
#[snafu(display("signature does not verify"))]
pub struct BadSignature;

/// The bytes of a signature, in the encoding TLS sends: DER for ECDSA.
///
/// Signatures up to the 104 bytes of a P-384 ECDSA signature are stored
/// inline; longer ones, such as RSA signatures, are stored on the heap.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Signature(SmallVec<[u8; Signature::INLINE_CAPACITY]>);

impl Signature {
    const INLINE_CAPACITY: usize = 104;

    /// Copy `bytes` into a new signature.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(SmallVec::from_slice(bytes))
    }

    /// The signature bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Signature {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Signature(")?;
        for byte in self.as_bytes() {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

/// A key that signs.
///
/// How the key is obtained is not part of the trait: a local private key is
/// built by its implementation's own constructor, and a hardware or remote
/// signer by its own means. The key's type determines the schemes it
/// produces. One signature is one interaction, carried by
/// [`SigningKey::Signing`].
pub trait SigningKey {
    /// A failure to sign.
    type Error;

    /// A signature in progress.
    type Signing<'a>: Future<Output = Result<Signature, Self::Error>> + 'a
    where
        Self: 'a;

    /// The schemes this key signs with.
    fn schemes(&self) -> &[SignatureScheme];

    /// Sign `message` with `scheme`.
    ///
    /// The future fails when `scheme` is not one of
    /// [`SigningKey::schemes`], or as the implementation documents.
    fn sign<'a>(&'a self, scheme: SignatureScheme, message: &'a [u8]) -> Self::Signing<'a>;
}

/// A signing key was asked for a scheme it does not sign with.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
#[snafu(display("signing key does not sign with {scheme:?}"))]
pub struct SchemeNotForKey {
    /// The scheme that was asked for.
    pub scheme: SignatureScheme,
}
