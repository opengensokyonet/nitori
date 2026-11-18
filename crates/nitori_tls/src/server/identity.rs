//! What a server presents to authenticate with a certificate.
//!
//! A server sends its certificate chain in Certificate and signs the
//! transcript in CertificateVerify with the key of the leaf. The scheme of
//! that signature must be one the client offered in
//! `signature_algorithms` and fit the key; RSA signs with RSASSA-PSS, and
//! no scheme uses SHA-1 (RFC 9846 §4.5.2).
//! [`CertificateSelection::select`] picks the chain and the scheme for
//! the selected target; the handshake computes the content to sign, and
//! the signature may wait.
//!
//! [`CertifiedKey`] presents one chain with one signing key;
//! [`CertifiedKey::from_pem_chain`] reads the chain from PEM.

use crate::{alert::AlertDescription, server::targets::SelectedTarget};
use bytes::Bytes;
use nitori_crypto::{SignatureScheme, SigningKey};
use rustls_pki_types::pem::{self, PemObject, SectionKind};
use snafu::{ResultExt, Snafu};

/// How a server picks the certificate chain it presents for a selected
/// target, and the scheme it signs CertificateVerify with.
///
/// `T` is the type of the selected target.
pub trait CertificateSelection<T> {
    /// The key that signs CertificateVerify.
    type Key: SigningKey;

    /// The chain and the scheme for `selected`, signing with a scheme of
    /// `offered`, the client's `signature_algorithms`.
    ///
    /// # Errors
    ///
    /// [`NoCommonSchemeError`] when the key signs with none of `offered`.
    fn select(
        &self,
        selected: &SelectedTarget<T>,
        offered: &[SignatureScheme],
    ) -> Result<Presented<'_, Self::Key>, NoCommonSchemeError>;
}

/// A certificate chain to send, with the key and scheme that sign
/// CertificateVerify.
#[derive(Debug)]
pub struct Presented<'a, K> {
    certificates: &'a [Bytes],
    key: &'a K,
    scheme: SignatureScheme,
}

impl<'a, K: SigningKey> Presented<'a, K> {
    /// The DER certificates, the leaf first.
    pub fn certificates(&self) -> &'a [Bytes] {
        self.certificates
    }

    /// The scheme CertificateVerify uses.
    pub fn scheme(&self) -> SignatureScheme {
        self.scheme
    }

    /// Signs `content`, the content of CertificateVerify, with the scheme.
    pub fn sign(&self, content: &'a [u8]) -> K::Signing<'a> {
        self.key.sign(self.scheme, content)
    }
}

/// The key signs with none of the schemes the client offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("no offered signature scheme fits the key"))]
pub struct NoCommonSchemeError;

impl NoCommonSchemeError {
    /// The alert the failure calls for: handshake_failure.
    pub fn alert(&self) -> AlertDescription {
        AlertDescription::HANDSHAKE_FAILURE
    }
}

/// Schemes CertificateVerify does not use: RSASSA-PKCS1-v1_5 and SHA-1
/// (RFC 9846 §4.3.3, §4.5.2).
const NOT_FOR_CERTIFICATE_VERIFY: &[u16] = &[0x0201, 0x0203, 0x0401, 0x0501, 0x0601];

/// One certificate chain and the signing key of its leaf, for every
/// target.
///
/// The scheme is the first of the key's schemes, in the key's order, that
/// the client offered.
#[derive(Debug)]
pub struct CertifiedKey<K> {
    certificates: Vec<Bytes>,
    key: K,
}

impl<K> CertifiedKey<K> {
    /// The chain `certificates`, DER certificates with the leaf first, and
    /// `key`, the leaf's key.
    ///
    /// # Errors
    ///
    /// [`EmptyChainError`] when `certificates` is empty.
    pub fn new(certificates: Vec<Bytes>, key: K) -> Result<Self, EmptyChainError> {
        if certificates.is_empty() {
            return Err(EmptyChainError);
        }
        Ok(Self { certificates, key })
    }

    /// The chain in `chain_pem`, PEM text such as the contents of a
    /// `cert.pem` file, and `key`, the leaf's key.
    ///
    /// The chain is the CERTIFICATE sections in the order of the text, so the
    /// text gives the leaf first; other sections, such as a private key in
    /// the same file, are skipped. Whether `key` belongs to the leaf is not
    /// checked.
    ///
    /// # Errors
    ///
    /// [`PemChainError::Malformed`] when the text is not valid PEM, and
    /// [`PemChainError::Empty`] when it has no CERTIFICATE section.
    pub fn from_pem_chain(chain_pem: &[u8], key: K) -> Result<Self, PemChainError> {
        let mut certificates = Vec::new();
        for section in <(SectionKind, Vec<u8>)>::pem_slice_iter(chain_pem) {
            let (kind, der) = section.context(MalformedSnafu)?;
            if kind == SectionKind::Certificate {
                certificates.push(Bytes::from(der));
            }
        }
        Self::new(certificates, key).context(EmptySnafu)
    }

    /// The certificates, the leaf first.
    pub fn certificates(&self) -> &[Bytes] {
        &self.certificates
    }

    /// The key.
    pub fn key(&self) -> &K {
        &self.key
    }
}

/// A [`CertifiedKey`] needs at least the leaf certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("certificate chain is empty"))]
pub struct EmptyChainError;

/// A certificate chain in PEM cannot make a [`CertifiedKey`].
#[derive(Debug, Snafu)]
pub enum PemChainError {
    /// The text is not valid PEM.
    #[snafu(display("malformed PEM"))]
    Malformed {
        /// The PEM reader's failure.
        source: pem::Error,
    },
    /// The text has no CERTIFICATE section.
    #[snafu(display("no certificate in PEM"))]
    Empty {
        /// The chain is empty.
        source: EmptyChainError,
    },
}

impl<K: SigningKey, T> CertificateSelection<T> for CertifiedKey<K> {
    type Key = K;

    fn select(
        &self,
        _selected: &SelectedTarget<T>,
        offered: &[SignatureScheme],
    ) -> Result<Presented<'_, K>, NoCommonSchemeError> {
        let scheme = self
            .key
            .schemes()
            .iter()
            .copied()
            .filter(|scheme| !NOT_FOR_CERTIFICATE_VERIFY.contains(&scheme.get()))
            .find(|scheme| offered.contains(scheme))
            .ok_or(NoCommonSchemeError)?;
        Ok(Presented {
            certificates: &self.certificates,
            key: &self.key,
            scheme,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CertificateSelection, CertifiedKey, EmptyChainError, NoCommonSchemeError, PemChainError,
    };
    use crate::{server::targets::SelectedTarget, test_support::ready_now};
    use bytes::Bytes;
    use core::{
        convert::Infallible,
        future::{Ready, ready},
    };
    use nitori_crypto::{Signature, SignatureScheme, SigningKey};

    /// A key that "signs" by echoing the scheme and the content.
    #[derive(Debug)]
    struct EchoKey(Vec<SignatureScheme>);

    impl SigningKey for EchoKey {
        type Error = Infallible;
        type Signing<'a> = Ready<Result<Signature, Infallible>>;

        fn schemes(&self) -> &[SignatureScheme] {
            &self.0
        }

        fn sign<'a>(&'a self, scheme: SignatureScheme, message: &'a [u8]) -> Self::Signing<'a> {
            let mut bytes = scheme.get().to_be_bytes().to_vec();
            bytes.extend_from_slice(message);
            ready(Ok(Signature::from_slice(&bytes)))
        }
    }

    fn selected() -> SelectedTarget<()> {
        SelectedTarget {
            target: (),
            application_protocol: None,
        }
    }

    fn identity(schemes: Vec<SignatureScheme>) -> CertifiedKey<EchoKey> {
        CertifiedKey::new(vec![Bytes::from_static(b"leaf")], EchoKey(schemes)).unwrap()
    }

    #[test]
    fn the_keys_first_offered_scheme_signs() {
        let identity = identity(vec![
            SignatureScheme::RSA_PSS_RSAE_SHA512,
            SignatureScheme::RSA_PSS_RSAE_SHA256,
        ]);
        let presented = identity
            .select(
                &selected(),
                &[
                    SignatureScheme::RSA_PSS_RSAE_SHA256,
                    SignatureScheme::RSA_PSS_RSAE_SHA512,
                ],
            )
            .unwrap();
        assert_eq!(presented.scheme(), SignatureScheme::RSA_PSS_RSAE_SHA512);
        assert_eq!(presented.certificates(), &[Bytes::from_static(b"leaf")]);
        let Ok(signature) = ready_now(presented.sign(b"content"));
        assert_eq!(signature.as_bytes(), b"\x08\x06content");
    }

    #[test]
    fn a_scheme_the_client_did_not_offer_or_pkcs1_is_not_used() {
        let identity = identity(vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_SECP256R1_SHA256,
        ]);
        let error = CertificateSelection::<()>::select(
            &identity,
            &selected(),
            &[SignatureScheme::RSA_PKCS1_SHA256, SignatureScheme::ED25519],
        )
        .unwrap_err();
        assert_eq!(error, NoCommonSchemeError);
        assert_eq!(
            error.alert(),
            crate::alert::AlertDescription::HANDSHAKE_FAILURE
        );
        assert_eq!(
            CertifiedKey::new(vec![], EchoKey(vec![])).unwrap_err(),
            EmptyChainError
        );
    }

    const LEAF: &[u8] = include_bytes!("../../testdata/docsrs/docsrs-com.der");
    const INTERMEDIATE: &[u8] = include_bytes!("../../testdata/docsrs/amazon-rsa-2048-m04.der");
    /// LEAF and INTERMEDIATE in PEM, in that order.
    const CHAIN_PEM: &[u8] = include_bytes!("../../testdata/docsrs/docsrs-com-chain.pem");
    /// The curve section that `openssl ecparam -genkey` writes before a key;
    /// the PEM reader skips labels it does not know.
    const EC_PARAMETERS: &[u8] =
        b"-----BEGIN EC PARAMETERS-----\nBggqhkjOPQMBBw==\n-----END EC PARAMETERS-----\n";
    /// A section the PEM reader knows and that is not a certificate, as when
    /// a file holds the key too. The contents need not be a key.
    const PRIVATE_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";

    #[test]
    fn a_pem_chain_keeps_the_certificates_in_order_and_skips_other_sections() {
        let pem = [EC_PARAMETERS, PRIVATE_KEY, CHAIN_PEM, PRIVATE_KEY].concat();
        let identity = CertifiedKey::from_pem_chain(&pem, EchoKey(vec![])).unwrap();
        assert_eq!(
            identity.certificates(),
            &[Bytes::from_static(LEAF), Bytes::from_static(INTERMEDIATE)]
        );
    }

    #[test]
    fn a_pem_chain_without_certificates_or_with_bad_base64_fails() {
        assert!(matches!(
            CertifiedKey::from_pem_chain(EC_PARAMETERS, EchoKey(vec![])),
            Err(PemChainError::Empty {
                source: EmptyChainError
            })
        ));
        let broken = b"-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        assert!(matches!(
            CertifiedKey::from_pem_chain(broken, EchoKey(vec![])),
            Err(PemChainError::Malformed { .. })
        ));
        let unterminated = b"-----BEGIN CERTIFICATE-----\nMIIF\n";
        assert!(matches!(
            CertifiedKey::from_pem_chain(unterminated, EchoKey(vec![])),
            Err(PemChainError::Malformed { .. })
        ));
    }
}
