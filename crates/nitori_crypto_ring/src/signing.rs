use core::future::{Ready, ready};

use nitori_crypto::{
    SchemeNotForKeyError, SchemeNotForKeySnafu, Signature, SignatureScheme, SigningKey,
};
use ring::{
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use rustls_pki_types::PrivateKeyDer;
use snafu::{OptionExt, ResultExt, Snafu, ensure};

fn check_scheme(
    schemes: &[SignatureScheme],
    scheme: SignatureScheme,
) -> Result<(), SchemeNotForKeyError> {
    ensure!(schemes.contains(&scheme), SchemeNotForKeySnafu { scheme });
    Ok(())
}

/// The curves an [`EcdsaSigningKey`] supports, with the scheme of each and
/// the DER of its AlgorithmIdentifier in PKCS#8: id-ecPublicKey with the
/// curve's name (RFC 5480 §2.1.1).
const ECDSA_CURVES: [(&signature::EcdsaSigningAlgorithm, &[SignatureScheme], &[u8]); 2] = [
    (
        &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        &[SignatureScheme::ECDSA_SECP256R1_SHA256],
        // id-ecPublicKey, secp256r1
        &[
            0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
            0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
        ],
    ),
    (
        &signature::ECDSA_P384_SHA384_ASN1_SIGNING,
        &[SignatureScheme::ECDSA_SECP384R1_SHA384],
        // id-ecPublicKey, secp384r1
        &[
            0x30, 0x10, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x05, 0x2b,
            0x81, 0x04, 0x00, 0x22,
        ],
    ),
];

/// A PKCS#8 document (RFC 5958 §2) holding `sec1`, an ECPrivateKey,
/// under `algorithm`, the DER AlgorithmIdentifier of its curve.
fn sec1_in_pkcs8(algorithm: &[u8], sec1: &[u8]) -> Vec<u8> {
    const VERSION_1: &[u8] = &[0x02, 0x01, 0x00];
    let mut private_key = Vec::new();
    push_der(&mut private_key, 0x04, &[sec1]);
    let mut pkcs8 = Vec::new();
    push_der(&mut pkcs8, 0x30, &[VERSION_1, algorithm, &private_key]);
    pkcs8
}

/// Appends the DER element of `tag` whose contents are `parts` in order.
fn push_der(out: &mut Vec<u8>, tag: u8, parts: &[&[u8]]) {
    let len: usize = parts.iter().map(|part| part.len()).sum();
    out.push(tag);
    if len < 0x80 {
        // Short form: the length itself.
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|&&byte| byte == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    for part in parts {
        out.extend_from_slice(part);
    }
}

/// A local ECDSA private key over P-256 or P-384.
///
/// It signs with the one scheme of its curve: ecdsa_secp256r1_sha256 or
/// ecdsa_secp384r1_sha384. Signing completes immediately.
#[derive(Debug)]
pub struct EcdsaSigningKey {
    key: signature::EcdsaKeyPair,
    schemes: &'static [SignatureScheme],
}

impl EcdsaSigningKey {
    /// The key in a PKCS#8 document, over P-256 or P-384.
    ///
    /// # Errors
    ///
    /// Fails when the document is not an ECDSA private key over either
    /// curve.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, EcdsaKeyRejectedError> {
        let random = SystemRandom::new();
        ECDSA_CURVES
            .iter()
            .find_map(|&(algorithm, schemes, _)| {
                signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8, &random)
                    .ok()
                    .map(|key| Self { key, schemes })
            })
            .context(EcdsaKeyRejectedSnafu)
    }

    /// The key in a PKCS#8 or a SEC1 document, as a PEM file gives it under
    /// the label PRIVATE KEY or EC PRIVATE KEY.
    ///
    /// ring needs the public key in the document: an ECPrivateKey without
    /// its optional publicKey field (RFC 5915 §3) is rejected.
    ///
    /// # Errors
    ///
    /// Fails when the document is in another format, or is not an ECDSA
    /// private key over a supported curve with its public key.
    pub fn from_der(key: &PrivateKeyDer<'_>) -> Result<Self, EcdsaKeyDerError> {
        match key {
            PrivateKeyDer::Pkcs8(key) => Ok(Self::from_pkcs8(key.secret_pkcs8_der())?),
            PrivateKeyDer::Sec1(key) => {
                Self::from_sec1(key.secret_sec1_der()).context(ecdsa_key_der_error::Sec1Snafu)
            }
            _ => ecdsa_key_der_error::UnsupportedFormatSnafu.fail(),
        }
    }

    /// ring reads only PKCS#8, so the SEC1 document is wrapped in one for
    /// each curve in turn; ring rejects a document whose own curve differs.
    fn from_sec1(sec1: &[u8]) -> Option<Self> {
        let random = SystemRandom::new();
        ECDSA_CURVES
            .iter()
            .find_map(|&(algorithm, schemes, curve)| {
                let pkcs8 = sec1_in_pkcs8(curve, sec1);
                signature::EcdsaKeyPair::from_pkcs8(algorithm, &pkcs8, &random)
                    .ok()
                    .map(|key| Self { key, schemes })
            })
    }

    /// The public key as an uncompressed point.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A private key document cannot make an [`EcdsaSigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum EcdsaKeyDerError {
    /// The document is neither PKCS#8 nor SEC1.
    #[snafu(display("private key document is neither pkcs#8 nor sec1"))]
    UnsupportedFormat,
    /// The PKCS#8 document is not a key over a supported curve.
    #[snafu(transparent)]
    Pkcs8 {
        /// The failure.
        source: EcdsaKeyRejectedError,
    },
    /// The SEC1 document is not a key over a supported curve with its
    /// public key.
    #[snafu(display(
        "sec1 document is not an ecdsa p-256 or p-384 private key with its public key"
    ))]
    Sec1,
}

/// A PKCS#8 document is not an ECDSA private key over a supported curve.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not an ecdsa p-256 or p-384 private key"))]
pub struct EcdsaKeyRejectedError;

/// A failure to sign with an [`EcdsaSigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum EcdsaSigningError {
    /// The key does not sign with the requested scheme.
    #[snafu(transparent)]
    Scheme {
        /// The failure.
        source: SchemeNotForKeyError,
    },
    /// ring failed to produce the signature, for example because its
    /// random source failed.
    #[snafu(display("ecdsa signing failed"))]
    Signing,
}

impl SigningKey for EcdsaSigningKey {
    type Error = EcdsaSigningError;
    type Signing<'a> = Ready<Result<Signature, EcdsaSigningError>>;

    fn schemes(&self) -> &[SignatureScheme] {
        self.schemes
    }

    fn sign<'a>(&'a self, scheme: SignatureScheme, message: &'a [u8]) -> Self::Signing<'a> {
        ready((|| {
            check_scheme(self.schemes, scheme)?;
            let signature = self
                .key
                .sign(&SystemRandom::new(), message)
                .ok()
                .context(ecdsa_signing_error::SigningSnafu)?;
            Ok(Signature::from_slice(signature.as_ref()))
        })())
    }
}

/// A local Ed25519 private key.
///
/// It signs with the ed25519 scheme. Signing completes immediately and
/// fails only for another scheme.
#[derive(Debug)]
pub struct Ed25519SigningKey {
    key: signature::Ed25519KeyPair,
}

const ED25519_SCHEMES: &[SignatureScheme] = &[SignatureScheme::ED25519];

impl Ed25519SigningKey {
    /// The key in a PKCS#8 v1 or v2 document.
    ///
    /// A v2 document's public key is checked against the private key; a v1
    /// document has none.
    ///
    /// # Errors
    ///
    /// Fails when the document is not an Ed25519 private key.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, Ed25519KeyRejectedError> {
        let key = signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8)
            .context(Ed25519KeyRejectedSnafu)?;
        Ok(Self { key })
    }

    /// The key in a PKCS#8 document, as a PEM file gives it under the label
    /// PRIVATE KEY; Ed25519 keys have no other format.
    ///
    /// # Errors
    ///
    /// Fails when the document is in another format, or is not an Ed25519
    /// private key.
    pub fn from_der(key: &PrivateKeyDer<'_>) -> Result<Self, Ed25519KeyDerError> {
        match key {
            PrivateKeyDer::Pkcs8(key) => Ok(Self::from_pkcs8(key.secret_pkcs8_der())?),
            _ => ed25519_key_der_error::UnsupportedFormatSnafu.fail(),
        }
    }

    /// The 32-byte public key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A private key document cannot make an [`Ed25519SigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum Ed25519KeyDerError {
    /// The document is not PKCS#8.
    #[snafu(display("private key document is not pkcs#8"))]
    UnsupportedFormat,
    /// The PKCS#8 document is not an Ed25519 key.
    #[snafu(transparent)]
    Pkcs8 {
        /// The failure.
        source: Ed25519KeyRejectedError,
    },
}

/// A PKCS#8 document is not an Ed25519 private key.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not an ed25519 private key"))]
pub struct Ed25519KeyRejectedError {
    source: ring::error::KeyRejected,
}

impl SigningKey for Ed25519SigningKey {
    type Error = SchemeNotForKeyError;
    type Signing<'a> = Ready<Result<Signature, SchemeNotForKeyError>>;

    fn schemes(&self) -> &[SignatureScheme] {
        ED25519_SCHEMES
    }

    fn sign<'a>(&'a self, scheme: SignatureScheme, message: &'a [u8]) -> Self::Signing<'a> {
        ready(
            check_scheme(ED25519_SCHEMES, scheme)
                .map(|()| Signature::from_slice(self.key.sign(message).as_ref())),
        )
    }
}

/// A local RSA private key with a modulus of 2048 to 8192 bits.
///
/// It signs with RSASSA-PSS, the only RSA signature TLS 1.3 allows in
/// handshake messages (RFC 8446 §4.4.3): rsa_pss_rsae_sha256,
/// rsa_pss_rsae_sha384 and rsa_pss_rsae_sha512. Signing completes
/// immediately.
#[derive(Debug)]
pub struct RsaSigningKey {
    key: signature::RsaKeyPair,
}

const RSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PSS_RSAE_SHA256,
    SignatureScheme::RSA_PSS_RSAE_SHA384,
    SignatureScheme::RSA_PSS_RSAE_SHA512,
];

/// The padding of each scheme in [`RSA_SCHEMES`].
fn rsa_padding(scheme: SignatureScheme) -> Option<&'static dyn signature::RsaEncoding> {
    Some(match scheme {
        SignatureScheme::RSA_PSS_RSAE_SHA256 => &signature::RSA_PSS_SHA256,
        SignatureScheme::RSA_PSS_RSAE_SHA384 => &signature::RSA_PSS_SHA384,
        SignatureScheme::RSA_PSS_RSAE_SHA512 => &signature::RSA_PSS_SHA512,
        _ => return None,
    })
}

impl RsaSigningKey {
    /// The key in a PKCS#8 document.
    ///
    /// # Errors
    ///
    /// Fails when the document is not an RSA private key of a supported
    /// size.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, RsaKeyRejectedError> {
        let key = signature::RsaKeyPair::from_pkcs8(pkcs8).context(RsaKeyRejectedSnafu)?;
        Ok(Self { key })
    }

    /// The key in a PKCS#8 or a PKCS#1 document, as a PEM file gives it
    /// under the label PRIVATE KEY or RSA PRIVATE KEY.
    ///
    /// # Errors
    ///
    /// Fails when the document is in another format, or is not an RSA
    /// private key of a supported size.
    pub fn from_der(key: &PrivateKeyDer<'_>) -> Result<Self, RsaKeyDerError> {
        match key {
            PrivateKeyDer::Pkcs8(key) => Ok(Self::from_pkcs8(key.secret_pkcs8_der())?),
            PrivateKeyDer::Pkcs1(key) => {
                let key = signature::RsaKeyPair::from_der(key.secret_pkcs1_der())
                    .context(rsa_key_der_error::Pkcs1Snafu)?;
                Ok(Self { key })
            }
            _ => rsa_key_der_error::UnsupportedFormatSnafu.fail(),
        }
    }

    /// The public key as a DER RSAPublicKey.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A private key document cannot make an [`RsaSigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RsaKeyDerError {
    /// The document is neither PKCS#8 nor PKCS#1.
    #[snafu(display("private key document is neither pkcs#8 nor pkcs#1"))]
    UnsupportedFormat,
    /// The PKCS#8 document is not an RSA key of a supported size.
    #[snafu(transparent)]
    Pkcs8 {
        /// The failure.
        source: RsaKeyRejectedError,
    },
    /// The PKCS#1 document is not an RSA key of a supported size.
    #[snafu(display("pkcs#1 document is not a supported rsa private key"))]
    Pkcs1 {
        /// ring's failure.
        source: ring::error::KeyRejected,
    },
}

/// A PKCS#8 document is not an RSA private key of a supported size.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not a supported rsa private key"))]
pub struct RsaKeyRejectedError {
    source: ring::error::KeyRejected,
}

/// A failure to sign with an [`RsaSigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RsaSigningError {
    /// The key does not sign with the requested scheme.
    #[snafu(transparent)]
    Scheme {
        /// The failure.
        source: SchemeNotForKeyError,
    },
    /// ring failed to produce the signature, for example because its
    /// random source failed.
    #[snafu(display("rsa signing failed"))]
    Signing,
}

impl SigningKey for RsaSigningKey {
    type Error = RsaSigningError;
    type Signing<'a> = Ready<Result<Signature, RsaSigningError>>;

    fn schemes(&self) -> &[SignatureScheme] {
        RSA_SCHEMES
    }

    fn sign<'a>(&'a self, scheme: SignatureScheme, message: &'a [u8]) -> Self::Signing<'a> {
        ready((|| {
            let padding = rsa_padding(scheme).context(SchemeNotForKeySnafu { scheme })?;
            let mut out = vec![0; self.key.public().modulus_len()];
            self.key
                .sign(padding, &SystemRandom::new(), message, &mut out)
                .ok()
                .context(rsa_signing_error::SigningSnafu)?;
            Ok(Signature::from_slice(&out))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::push_der;

    #[test]
    fn der_lengths_use_the_shortest_form() {
        for (len, header) in [
            (0x7f, &[0x04, 0x7f][..]),
            (0x80, &[0x04, 0x81, 0x80]),
            (0xff, &[0x04, 0x81, 0xff]),
            (0x100, &[0x04, 0x82, 0x01, 0x00]),
            (0x1_0000, &[0x04, 0x83, 0x01, 0x00, 0x00]),
        ] {
            let contents = vec![0xaa; len];
            let mut out = Vec::new();
            push_der(&mut out, 0x04, &[&contents[..1], &contents[1..]]);
            assert_eq!(&out[..header.len()], header, "{len:#x}");
            assert_eq!(&out[header.len()..], &contents[..]);
        }
    }
}
