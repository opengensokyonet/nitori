use core::future::{Ready, ready};

use nitori_crypto::{
    SchemeNotForKey, SchemeNotForKeySnafu, Signature, SignatureScheme, SigningKey,
};
use ring::{
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use snafu::{OptionExt, ResultExt, Snafu, ensure};

fn check_scheme(
    schemes: &[SignatureScheme],
    scheme: SignatureScheme,
) -> Result<(), SchemeNotForKey> {
    ensure!(schemes.contains(&scheme), SchemeNotForKeySnafu { scheme });
    Ok(())
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
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, EcdsaKeyRejected> {
        const CURVES: [(&signature::EcdsaSigningAlgorithm, &[SignatureScheme]); 2] = [
            (
                &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                &[SignatureScheme::ECDSA_SECP256R1_SHA256],
            ),
            (
                &signature::ECDSA_P384_SHA384_ASN1_SIGNING,
                &[SignatureScheme::ECDSA_SECP384R1_SHA384],
            ),
        ];
        let random = SystemRandom::new();
        CURVES
            .iter()
            .find_map(|&(algorithm, schemes)| {
                signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8, &random)
                    .ok()
                    .map(|key| Self { key, schemes })
            })
            .context(EcdsaKeyRejectedSnafu)
    }

    /// The public key as an uncompressed point.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A PKCS#8 document is not an ECDSA private key over a supported curve.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not an ecdsa p-256 or p-384 private key"))]
pub struct EcdsaKeyRejected;

/// A failure to sign with an [`EcdsaSigningKey`].
#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum EcdsaSigningError {
    /// The key does not sign with the requested scheme.
    #[snafu(transparent)]
    Scheme {
        /// The failure.
        source: SchemeNotForKey,
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
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, Ed25519KeyRejected> {
        let key = signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8)
            .context(Ed25519KeyRejectedSnafu)?;
        Ok(Self { key })
    }

    /// The 32-byte public key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A PKCS#8 document is not an Ed25519 private key.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not an ed25519 private key"))]
pub struct Ed25519KeyRejected {
    source: ring::error::KeyRejected,
}

impl SigningKey for Ed25519SigningKey {
    type Error = SchemeNotForKey;
    type Signing<'a> = Ready<Result<Signature, SchemeNotForKey>>;

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
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, RsaKeyRejected> {
        let key = signature::RsaKeyPair::from_pkcs8(pkcs8).context(RsaKeyRejectedSnafu)?;
        Ok(Self { key })
    }

    /// The public key as a DER RSAPublicKey.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        self.key.public_key().as_ref()
    }
}

/// A PKCS#8 document is not an RSA private key of a supported size.
#[derive(Debug, Snafu)]
#[snafu(display("pkcs#8 document is not a supported rsa private key"))]
pub struct RsaKeyRejected {
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
        source: SchemeNotForKey,
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
