//! The public key a peer authenticates with, from its SubjectPublicKeyInfo.

use super::der::{self, MalformedDerError, Reader};
use crate::alert::AlertDescription;
use bytes::Bytes;
use nitori_crypto::SignatureScheme;
use snafu::{ResultExt, Snafu};

/// The algorithm of a public key, from the algorithm identifier of its
/// SubjectPublicKeyInfo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublicKeyAlgorithm {
    /// id-ecPublicKey over P-256.
    EcdsaP256,
    /// id-ecPublicKey over P-384.
    EcdsaP384,
    /// id-ecPublicKey over P-521.
    EcdsaP521,
    /// Ed25519 (RFC 8410).
    Ed25519,
    /// rsaEncryption.
    Rsa,
}

/// id-ecPublicKey, 1.2.840.10045.2.1.
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// prime256v1, 1.2.840.10045.3.1.7.
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// secp384r1, 1.3.132.0.34.
const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
/// secp521r1, 1.3.132.0.35.
const P521: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x23];
/// id-Ed25519, 1.3.101.112.
const ED25519: &[u8] = &[0x2b, 0x65, 0x70];
/// rsaEncryption, 1.2.840.113549.1.1.1.
const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/// A public key: its algorithm and the contents of its `subjectPublicKey`
/// bit string, the form signature verification takes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SubjectPublicKey {
    algorithm: PublicKeyAlgorithm,
    key: Bytes,
}

impl SubjectPublicKey {
    /// The key of the DER SubjectPublicKeyInfo `spki`.
    ///
    /// # Errors
    ///
    /// [`InvalidPublicKeyError`] when `spki` is not a SubjectPublicKeyInfo or its
    /// algorithm is not one of [`PublicKeyAlgorithm`].
    pub fn from_spki(spki: &[u8]) -> Result<Self, InvalidPublicKeyError> {
        use invalid_public_key::*;
        let (algorithm, key) = parse(spki).context(MalformedSnafu)?;
        let algorithm = algorithm.ok_or(InvalidPublicKeyError::UnsupportedAlgorithm)?;
        Ok(Self {
            algorithm,
            key: Bytes::copy_from_slice(key),
        })
    }

    /// The algorithm.
    pub fn algorithm(&self) -> PublicKeyAlgorithm {
        self.algorithm
    }

    /// The contents of the `subjectPublicKey` bit string: an uncompressed
    /// point for ECDSA, 32 bytes for Ed25519, a DER RSAPublicKey for RSA.
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// Checks that a CertificateVerify signed with `scheme` can come from
    /// this key: an ECDSA scheme of the key's curve, Ed25519 for an Ed25519
    /// key, and RSASSA-PSS with an rsaEncryption key for RSA, since a
    /// CertificateVerify does not use RSASSA-PKCS1-v1_5 (RFC 9846 §4.5.2).
    ///
    /// # Errors
    ///
    /// [`SchemeKeyMismatchError`] otherwise.
    pub fn check_certificate_verify_scheme(
        &self,
        scheme: SignatureScheme,
    ) -> Result<(), SchemeKeyMismatchError> {
        let matches = match self.algorithm {
            PublicKeyAlgorithm::EcdsaP256 => scheme == SignatureScheme::ECDSA_SECP256R1_SHA256,
            PublicKeyAlgorithm::EcdsaP384 => scheme == SignatureScheme::ECDSA_SECP384R1_SHA384,
            PublicKeyAlgorithm::EcdsaP521 => scheme == SignatureScheme::ECDSA_SECP521R1_SHA512,
            PublicKeyAlgorithm::Ed25519 => scheme == SignatureScheme::ED25519,
            PublicKeyAlgorithm::Rsa => [
                SignatureScheme::RSA_PSS_RSAE_SHA256,
                SignatureScheme::RSA_PSS_RSAE_SHA384,
                SignatureScheme::RSA_PSS_RSAE_SHA512,
            ]
            .contains(&scheme),
        };
        if !matches {
            return Err(SchemeKeyMismatchError {
                scheme,
                algorithm: self.algorithm,
            });
        }
        Ok(())
    }
}

/// The algorithm, when it is one of [`PublicKeyAlgorithm`], and the key
/// bytes of a SubjectPublicKeyInfo.
fn parse(spki: &[u8]) -> Result<(Option<PublicKeyAlgorithm>, &[u8]), MalformedDerError> {
    let mut fields = Reader::new(Reader::single(spki, der::SEQUENCE)?);
    let mut identifier = Reader::new(fields.expect(der::SEQUENCE)?.contents);
    let bits = fields.expect(der::BIT_STRING)?.contents;
    if !fields.is_empty() {
        return Err(MalformedDerError);
    }
    let (&unused, key) = bits.split_first().ok_or(MalformedDerError)?;
    if unused != 0 {
        return Err(MalformedDerError);
    }
    let oid = identifier.expect(der::OBJECT_IDENTIFIER)?.contents;
    let parameters = identifier.rest();
    let algorithm = match oid {
        EC_PUBLIC_KEY => match Reader::single(parameters, der::OBJECT_IDENTIFIER)? {
            P256 => Some(PublicKeyAlgorithm::EcdsaP256),
            P384 => Some(PublicKeyAlgorithm::EcdsaP384),
            P521 => Some(PublicKeyAlgorithm::EcdsaP521),
            _ => None,
        },
        ED25519 if parameters.is_empty() => Some(PublicKeyAlgorithm::Ed25519),
        // The parameters of rsaEncryption are NULL (RFC 3279 §2.3.1).
        RSA_ENCRYPTION if Reader::single(parameters, der::NULL)?.is_empty() => {
            Some(PublicKeyAlgorithm::Rsa)
        }
        _ => None,
    };
    Ok((algorithm, key))
}

/// A SubjectPublicKeyInfo does not give a key this crate uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(module(invalid_public_key), visibility(pub(crate)))]
pub enum InvalidPublicKeyError {
    /// The bytes are not a SubjectPublicKeyInfo.
    #[snafu(display("SubjectPublicKeyInfo malformed"))]
    Malformed {
        /// The failure.
        source: MalformedDerError,
    },
    /// The key's algorithm is not one of [`PublicKeyAlgorithm`].
    #[snafu(display("public key algorithm not supported"))]
    UnsupportedAlgorithm,
}

/// A CertificateVerify scheme does not fit the key it is verified with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("{scheme:?} does not sign with a key of {algorithm:?}"))]
pub struct SchemeKeyMismatchError {
    scheme: SignatureScheme,
    algorithm: PublicKeyAlgorithm,
}

impl SchemeKeyMismatchError {
    /// The scheme of the CertificateVerify.
    pub fn scheme(self) -> SignatureScheme {
        self.scheme
    }

    /// The algorithm of the key.
    pub fn algorithm(self) -> PublicKeyAlgorithm {
        self.algorithm
    }

    /// The alert the failure calls for: illegal_parameter, a field that
    /// does not agree with another (RFC 9846 §6.2).
    pub fn alert(&self) -> AlertDescription {
        AlertDescription::ILLEGAL_PARAMETER
    }
}

#[cfg(test)]
mod tests {
    use super::{InvalidPublicKeyError, PublicKeyAlgorithm, SubjectPublicKey};
    use crate::verify::der::MalformedDerError;
    use nitori_crypto::SignatureScheme;

    const XENON_KEY: &[u8] = include_bytes!("../../testdata/docsrs/google-xenon2027h1.spki.der");

    #[test]
    fn an_ecdsa_key_parses_with_its_curve() {
        let key = SubjectPublicKey::from_spki(XENON_KEY).unwrap();
        assert_eq!(key.algorithm(), PublicKeyAlgorithm::EcdsaP256);
        assert_eq!(key.key().len(), 65);
        assert_eq!(key.key()[0], 0x04);
        assert!(
            key.check_certificate_verify_scheme(SignatureScheme::ECDSA_SECP256R1_SHA256)
                .is_ok()
        );
        let error = key
            .check_certificate_verify_scheme(SignatureScheme::ECDSA_SECP384R1_SHA384)
            .unwrap_err();
        assert_eq!(error.algorithm(), PublicKeyAlgorithm::EcdsaP256);
    }

    #[test]
    fn rsa_and_ed25519_keys_parse() {
        let mut rsa = vec![0x30, 0x12, 0x30, 0x0d, 0x06, 0x09];
        rsa.extend_from_slice(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]);
        rsa.extend_from_slice(&[0x05, 0x00, 0x03, 0x01, 0x00]);
        let key = SubjectPublicKey::from_spki(&rsa).unwrap();
        assert_eq!(key.algorithm(), PublicKeyAlgorithm::Rsa);
        assert!(
            key.check_certificate_verify_scheme(SignatureScheme::RSA_PSS_RSAE_SHA384)
                .is_ok()
        );
        assert!(
            key.check_certificate_verify_scheme(SignatureScheme::RSA_PKCS1_SHA256)
                .is_err()
        );
        let mut ed25519 = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        ed25519.extend_from_slice(&[7; 32]);
        let key = SubjectPublicKey::from_spki(&ed25519).unwrap();
        assert_eq!(key.algorithm(), PublicKeyAlgorithm::Ed25519);
        assert_eq!(key.key(), &[7; 32]);
    }

    #[test]
    fn other_keys_are_rejected() {
        // Ed448, 1.3.101.113.
        let ed448 = [
            0x30, 0x0a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x71, 0x03, 0x01, 0x00,
        ];
        assert_eq!(
            SubjectPublicKey::from_spki(&ed448).unwrap_err(),
            InvalidPublicKeyError::UnsupportedAlgorithm
        );
        // A bit string with unused bits.
        let unused = [
            0x30, 0x0a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x01, 0x01,
        ];
        assert_eq!(
            SubjectPublicKey::from_spki(&unused).unwrap_err(),
            InvalidPublicKeyError::Malformed {
                source: MalformedDerError
            }
        );
        let mut trailing = XENON_KEY.to_vec();
        trailing.push(0);
        assert!(SubjectPublicKey::from_spki(&trailing).is_err());
    }
}
