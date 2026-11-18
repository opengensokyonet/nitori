//! Verification of a server's certificate chain.
//!
//! A client must authenticate the server (RFC 9001 §4.4). On a plain Web
//! connection that means: build a path from the server's certificate to a
//! trusted root, check validity, the serverAuth purpose and the name
//! against the target, and verify the signatures. A [`CertificateVerifier`]
//! does this and hands back the [`SubjectPublicKey`] of the server's
//! certificate, with which the handshake verifies CertificateVerify.
//!
//! [`WebPkiVerifier`] builds and checks the path with rustls-webpki and
//! verifies signatures with the backend. Four parts of it can be replaced,
//! each called on a path that webpki has verified up to a trust anchor,
//! where a refusal makes webpki try another path:
//!
//! - [`TrustAnchors`]: where the roots come from; one verification uses one
//!   snapshot of them. [`AnchorStore`] holds roots loaded at run time.
//! - [`Distrust`]: public keys refused wherever they appear in a path.
//!   [`SpkiDistrustList`] holds a list loaded at run time.
//! - [`Revocation`]: revocation. [`NoRevocationCheck`] does not check.
//! - [`Transparency`]: Certificate Transparency.
//!   [`transparency::EmbeddedSctPolicy`] requires embedded SCTs by Chrome's
//!   rules.
//!
//! The time comes from a [`Clock`].
//!
//! The parts are synchronous, with their data local; a verifier that must
//! wait, for example to fetch revocation data, implements
//! [`CertificateVerifier`] itself.

use crate::{
    alert::AlertDescription,
    client::{ServerTarget, offering::VerifierSupport},
    clock::Clock,
    message::certificate::CertificateEntry,
};
use bytes::Bytes;
use core::{
    cell::RefCell,
    convert::Infallible,
    future::{Future, Ready, ready},
    time::Duration,
};
use nitori_crypto::{Hash, SignatureScheme, SignatureVerify, Supported};
use rustls_pki_types::{
    AlgorithmIdentifier, CertificateDer, InvalidSignature, ServerName,
    SignatureVerificationAlgorithm, TrustAnchor, UnixTime, alg_id,
};
use snafu::{ResultExt, Snafu};
use std::{
    collections::BTreeSet,
    sync::{Arc, PoisonError, RwLock},
};
use time::UtcDateTime;
use webpki::{EndEntityCert, KeyUsage, VerifiedPath};

mod der;
pub mod public_key;
pub mod transparency;

#[cfg(test)]
mod tests;

pub use der::MalformedDerError;
pub use public_key::SubjectPublicKey;

/// How a client verifies the certificate chain of a server.
pub trait CertificateVerifier {
    /// A failure of verification.
    type Error;

    /// Verification in progress.
    type Verifying<'a>: Future<Output = Result<SubjectPublicKey, Self::Error>> + 'a
    where
        Self: 'a;

    /// What the verifier accepts, for the ClientHello to state.
    fn support(&self) -> VerifierSupport;

    /// Verifies `certificates`, the entries of the server's Certificate
    /// message, leaf first, for `target`: the public key of the leaf.
    fn verify<'a>(
        &'a self,
        target: &'a ServerTarget,
        certificates: &'a [CertificateEntry<Bytes>],
    ) -> Self::Verifying<'a>;
}

/// Where the trust anchors of a verification come from.
pub trait TrustAnchors {
    /// The anchors at one moment.
    type Snapshot<'a>: AsRef<[TrustAnchor<'static>]>
    where
        Self: 'a;

    /// The anchors now; a later update does not change a snapshot taken.
    fn snapshot(&self) -> Self::Snapshot<'_>;
}

/// Public keys refused wherever they appear in a path.
pub trait Distrust {
    /// Whether `spki`, a DER SubjectPublicKeyInfo, is refused.
    fn contains(&self, spki: &[u8]) -> bool;
}

/// A revocation check of a verified path.
pub trait Revocation {
    /// A failure: the path is revoked, or its status cannot be told.
    type Error: core::error::Error + 'static;

    /// Checks `path`.
    ///
    /// # Errors
    ///
    /// As the implementation documents.
    fn check(&self, path: &VerifiedPath<'_>) -> Result<(), Self::Error>;
}

/// A Certificate Transparency check of a leaf certificate, on the backend
/// `C`.
pub trait Transparency<C> {
    /// A failure.
    type Error: core::error::Error + 'static;

    /// Checks `leaf`, a DER certificate, issued by the holder of
    /// `issuer_spki`, a DER SubjectPublicKeyInfo, at `now`.
    ///
    /// # Errors
    ///
    /// As the implementation documents.
    fn check(
        &self,
        backend: &C,
        leaf: &[u8],
        issuer_spki: &[u8],
        now: UtcDateTime,
    ) -> Result<(), Self::Error>;
}

/// Trust anchors loaded at run time, replaced as a whole.
#[derive(Debug, Default)]
pub struct AnchorStore {
    anchors: RwLock<Arc<[TrustAnchor<'static>]>>,
}

/// The anchors of an [`AnchorStore`] at one moment.
#[derive(Clone, Debug)]
pub struct AnchorSnapshot(Arc<[TrustAnchor<'static>]>);

impl AsRef<[TrustAnchor<'static>]> for AnchorSnapshot {
    fn as_ref(&self) -> &[TrustAnchor<'static>] {
        &self.0
    }
}

impl AnchorStore {
    /// The anchors of `certificates`, DER certificates of trusted roots.
    ///
    /// # Errors
    ///
    /// [`InvalidAnchorError`] for a certificate webpki cannot read as an anchor.
    pub fn from_certificates<'a>(
        certificates: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, InvalidAnchorError> {
        Ok(Self {
            anchors: RwLock::new(anchors(certificates)?),
        })
    }

    /// Replaces the anchors with those of `certificates`; verifications in
    /// progress keep their snapshot.
    ///
    /// # Errors
    ///
    /// [`InvalidAnchorError`] for a certificate webpki cannot read as an anchor;
    /// the anchors are then left as they were.
    pub fn replace<'a>(
        &self,
        certificates: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<(), InvalidAnchorError> {
        let anchors = anchors(certificates)?;
        *self.anchors.write().unwrap_or_else(PoisonError::into_inner) = anchors;
        Ok(())
    }
}

fn anchors<'a>(
    certificates: impl IntoIterator<Item = &'a [u8]>,
) -> Result<Arc<[TrustAnchor<'static>]>, InvalidAnchorError> {
    certificates
        .into_iter()
        .enumerate()
        .map(|(index, der)| {
            webpki::anchor_from_trusted_cert(&CertificateDer::from(der))
                .map(|anchor| anchor.to_owned())
                .context(InvalidAnchorSnafu { index })
        })
        .collect()
}

impl TrustAnchors for AnchorStore {
    type Snapshot<'a> = AnchorSnapshot;

    fn snapshot(&self) -> AnchorSnapshot {
        AnchorSnapshot(Arc::clone(
            &self.anchors.read().unwrap_or_else(PoisonError::into_inner),
        ))
    }
}

/// A certificate given as a trust anchor cannot be read as one.
#[derive(Debug, Snafu)]
#[snafu(display("trust anchor {index} invalid"))]
pub struct InvalidAnchorError {
    index: usize,
    source: webpki::Error,
}

impl InvalidAnchorError {
    /// The position of the certificate among those given.
    pub fn index(&self) -> usize {
        self.index
    }
}

/// Public keys refused by their DER SubjectPublicKeyInfo, loaded and
/// updated at run time.
#[derive(Debug, Default)]
pub struct SpkiDistrustList {
    keys: RwLock<BTreeSet<Box<[u8]>>>,
}

impl SpkiDistrustList {
    /// The list of `keys`, DER SubjectPublicKeyInfos.
    pub fn new<'a>(keys: impl IntoIterator<Item = &'a [u8]>) -> Self {
        Self {
            keys: RwLock::new(keys.into_iter().map(Box::from).collect()),
        }
    }

    /// Adds `key`.
    pub fn insert(&self, key: &[u8]) {
        self.keys
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(Box::from(key));
    }

    /// Replaces the list with `keys`.
    pub fn replace<'a>(&self, keys: impl IntoIterator<Item = &'a [u8]>) {
        *self.keys.write().unwrap_or_else(PoisonError::into_inner) =
            keys.into_iter().map(Box::from).collect();
    }
}

impl Distrust for SpkiDistrustList {
    fn contains(&self, spki: &[u8]) -> bool {
        self.keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(spki)
    }
}

/// A revocation check that checks nothing: every path passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NoRevocationCheck;

impl Revocation for NoRevocationCheck {
    type Error = Infallible;

    fn check(&self, _path: &VerifiedPath<'_>) -> Result<(), Infallible> {
        Ok(())
    }
}

/// The certificate signature algorithms the verifier maps to TLS
/// signature schemes, with the algorithm identifiers of the key and the
/// signature in a certificate, most preferred first.
const ALGORITHMS: &[(SignatureScheme, AlgorithmIdentifier, AlgorithmIdentifier)] = &[
    (
        SignatureScheme::ECDSA_SECP256R1_SHA256,
        alg_id::ECDSA_P256,
        alg_id::ECDSA_SHA256,
    ),
    (
        SignatureScheme::ECDSA_SECP384R1_SHA384,
        alg_id::ECDSA_P384,
        alg_id::ECDSA_SHA384,
    ),
    (
        SignatureScheme::ECDSA_SECP521R1_SHA512,
        alg_id::ECDSA_P521,
        alg_id::ECDSA_SHA512,
    ),
    (SignatureScheme::ED25519, alg_id::ED25519, alg_id::ED25519),
    (
        SignatureScheme::RSA_PSS_RSAE_SHA256,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PSS_SHA256,
    ),
    (
        SignatureScheme::RSA_PSS_RSAE_SHA384,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PSS_SHA384,
    ),
    (
        SignatureScheme::RSA_PSS_RSAE_SHA512,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PSS_SHA512,
    ),
    (
        SignatureScheme::RSA_PKCS1_SHA256,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PKCS1_SHA256,
    ),
    (
        SignatureScheme::RSA_PKCS1_SHA384,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PKCS1_SHA384,
    ),
    (
        SignatureScheme::RSA_PKCS1_SHA512,
        alg_id::RSA_ENCRYPTION,
        alg_id::RSA_PKCS1_SHA512,
    ),
];

/// One certificate signature algorithm for webpki, verified by the
/// backend.
///
/// webpki takes its algorithms as `&dyn SignatureVerificationAlgorithm`;
/// that erasure is webpki's, and this type only adapts the backend to it.
struct BackendAlgorithm<'a, C: SignatureVerify> {
    backend: &'a C,
    scheme: Supported<C, SignatureScheme>,
    public_key: AlgorithmIdentifier,
    signature: AlgorithmIdentifier,
}

impl<C: SignatureVerify> core::fmt::Debug for BackendAlgorithm<'_, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("BackendAlgorithm")
            .field(&self.scheme)
            .finish()
    }
}

impl<C: SignatureVerify + Sync> SignatureVerificationAlgorithm for BackendAlgorithm<'_, C> {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        SignatureVerify::verify(self.backend, self.scheme, public_key, message, signature)
            .map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.public_key
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature
    }
}

/// Where in a path a certificate sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathPosition {
    /// The server's own certificate.
    EndEntity,
    /// An intermediate, by its distance from the end entity: 1 for the
    /// end entity's issuer.
    Intermediate {
        /// The distance.
        depth: usize,
    },
    /// The trust anchor.
    Anchor,
}

/// Verification with rustls-webpki, signatures verified by the backend `C`,
/// with the roots of `A`, the refused keys of `D`, the revocation check of
/// `R`, the CT check of `T` and the time of `K`.
///
/// It checks the path to a root, the validity at the clock's time, the
/// serverAuth purpose, and the name of the target against the end entity
/// certificate. A path that webpki verified is then refused when a key in
/// it is distrusted, or the revocation or CT check fails, and webpki tries
/// another path; when none passes, the error is that of the last path
/// refused this way, or webpki's own when there was none.
///
/// Certificate signatures are those of ECDSA over P-256 with SHA-256, P-384
/// with SHA-384 and P-521 with SHA-512, Ed25519, RSASSA-PSS and
/// RSASSA-PKCS1-v1_5, as far as the backend verifies their TLS schemes.
#[derive(Debug)]
pub struct WebPkiVerifier<C, A, D, R, T, K> {
    backend: C,
    anchors: A,
    distrust: D,
    revocation: R,
    transparency: T,
    clock: K,
}

impl<C, A, D, R, T, K> WebPkiVerifier<C, A, D, R, T, K> {
    /// The verifier of these parts.
    pub fn new(
        backend: C,
        anchors: A,
        distrust: D,
        revocation: R,
        transparency: T,
        clock: K,
    ) -> Self {
        Self {
            backend,
            anchors,
            distrust,
            revocation,
            transparency,
            clock,
        }
    }

    /// The trust anchors, for updating them.
    pub fn anchors(&self) -> &A {
        &self.anchors
    }

    /// The distrusted keys, for updating them.
    pub fn distrust(&self) -> &D {
        &self.distrust
    }

    /// The CT check, for updating its data.
    pub fn transparency(&self) -> &T {
        &self.transparency
    }
}

/// Why a path webpki verified was refused.
enum Refusal<R, T> {
    Distrusted(PathPosition),
    Revocation(R),
    Transparency(T),
}

impl<C, A, D, R, T, K> WebPkiVerifier<C, A, D, R, T, K>
where
    C: Hash + SignatureVerify + Sync,
    A: TrustAnchors,
    D: Distrust,
    R: Revocation,
    T: Transparency<C>,
    K: Clock,
{
    /// Verifies `certificates`, leaf first, for `target`.
    ///
    /// # Errors
    ///
    /// [`VerifyCertificateChainError`].
    pub fn verify_now(
        &self,
        target: &ServerTarget,
        certificates: &[CertificateEntry<Bytes>],
    ) -> Result<SubjectPublicKey, VerifyCertificateChainError<R::Error, T::Error>> {
        use verify_certificate_chain_error::*;
        let (leaf, intermediates) = certificates
            .split_first()
            .ok_or(VerifyCertificateChainError::NoCertificate)?;
        let leaf_der = CertificateDer::from(leaf.data().as_ref());
        let end_entity = EndEntityCert::try_from(&leaf_der).context(EndEntitySnafu)?;
        let intermediates: Vec<CertificateDer<'_>> = intermediates
            .iter()
            .map(|entry| CertificateDer::from(entry.data().as_ref()))
            .collect();
        let algorithms: Vec<BackendAlgorithm<'_, C>> = ALGORITHMS
            .iter()
            .filter_map(|&(scheme, public_key, signature)| {
                Some(BackendAlgorithm {
                    backend: &self.backend,
                    scheme: self.backend.signature_scheme(scheme)?,
                    public_key,
                    signature,
                })
            })
            .collect();
        let algorithms: Vec<&dyn SignatureVerificationAlgorithm> = algorithms
            .iter()
            .map(|algorithm| algorithm as &dyn SignatureVerificationAlgorithm)
            .collect();
        let anchors = self.anchors.snapshot();
        let now = self.clock.now();
        // Years before 1970 are past every certificate's validity.
        let since_epoch =
            Duration::from_secs(u64::try_from(now.unix_timestamp()).unwrap_or_default());
        let refusal = RefCell::new(None);
        let check = |path: &VerifiedPath<'_>| -> Result<(), webpki::Error> {
            match self.check_path(path, leaf.data(), now) {
                Ok(()) => Ok(()),
                Err(reason) => {
                    *refusal.borrow_mut() = Some(reason);
                    // The kind is not read: the refusal above is reported.
                    Err(webpki::Error::CertDistrusted)
                }
            }
        };
        let verified = end_entity.verify_for_usage(
            &algorithms,
            anchors.as_ref(),
            &intermediates,
            UnixTime::since_unix_epoch(since_epoch),
            KeyUsage::server_auth(),
            None,
            Some(&check),
        );
        if let Err(error) = verified {
            return Err(match refusal.into_inner() {
                Some(Refusal::Distrusted(position)) => {
                    VerifyCertificateChainError::Distrusted { position }
                }
                Some(Refusal::Revocation(source)) => {
                    VerifyCertificateChainError::Revocation { source }
                }
                Some(Refusal::Transparency(source)) => {
                    VerifyCertificateChainError::Transparency { source }
                }
                None => VerifyCertificateChainError::Path { source: error },
            });
        }
        end_entity
            .verify_is_valid_for_subject_name(&ServerName::DnsName(target.dns_name().clone()))
            .context(NameSnafu)?;
        SubjectPublicKey::from_spki(end_entity.subject_public_key_info().as_ref())
            .context(LeafKeySnafu)
    }

    fn check_path(
        &self,
        path: &VerifiedPath<'_>,
        leaf: &Bytes,
        now: UtcDateTime,
    ) -> Result<(), Refusal<R::Error, T::Error>> {
        let anchor_spki = der::wrap(
            der::SEQUENCE,
            path.anchor().subject_public_key_info.as_ref(),
        );
        let leaf_spki = path.end_entity().subject_public_key_info();
        let intermediate_spkis: Vec<_> = path
            .intermediate_certificates()
            .map(|certificate| certificate.subject_public_key_info())
            .collect();
        if self.distrust.contains(leaf_spki.as_ref()) {
            return Err(Refusal::Distrusted(PathPosition::EndEntity));
        }
        for (index, spki) in intermediate_spkis.iter().enumerate() {
            if self.distrust.contains(spki.as_ref()) {
                return Err(Refusal::Distrusted(PathPosition::Intermediate {
                    depth: index + 1,
                }));
            }
        }
        if self.distrust.contains(&anchor_spki) {
            return Err(Refusal::Distrusted(PathPosition::Anchor));
        }
        self.revocation.check(path).map_err(Refusal::Revocation)?;
        let issuer_spki = intermediate_spkis
            .first()
            .map_or(&anchor_spki[..], |spki| spki.as_ref());
        self.transparency
            .check(&self.backend, leaf, issuer_spki, now)
            .map_err(Refusal::Transparency)
    }
}

impl<C, A, D, R, T, K> CertificateVerifier for WebPkiVerifier<C, A, D, R, T, K>
where
    C: Hash + SignatureVerify + Sync,
    A: TrustAnchors,
    D: Distrust,
    R: Revocation,
    T: Transparency<C>,
    K: Clock,
{
    type Error = VerifyCertificateChainError<R::Error, T::Error>;
    type Verifying<'a>
        = Ready<Result<SubjectPublicKey, Self::Error>>
    where
        Self: 'a;

    fn support(&self) -> VerifierSupport {
        let schemes = ALGORITHMS
            .iter()
            .map(|&(scheme, _, _)| scheme)
            .filter(|&scheme| self.backend.signature_scheme(scheme).is_some())
            .collect();
        VerifierSupport::new(schemes, false)
    }

    fn verify<'a>(
        &'a self,
        target: &'a ServerTarget,
        certificates: &'a [CertificateEntry<Bytes>],
    ) -> Self::Verifying<'a> {
        ready(self.verify_now(target, certificates))
    }
}

/// A server's certificate chain does not authenticate it.
///
/// `R` and `T` are the failures of the revocation and CT checks.
#[derive(Debug, Snafu)]
#[snafu(module(verify_certificate_chain_error), visibility(pub(crate)))]
pub enum VerifyCertificateChainError<R, T>
where
    R: core::error::Error + 'static,
    T: core::error::Error + 'static,
{
    /// The Certificate message has no certificate.
    #[snafu(display("server sent no certificate"))]
    NoCertificate,
    /// The server's certificate cannot be read.
    #[snafu(display("end entity certificate invalid"))]
    EndEntity {
        /// webpki's failure.
        source: webpki::Error,
    },
    /// No path to a trust anchor verifies.
    #[snafu(display("no valid certificate path"))]
    Path {
        /// webpki's failure.
        source: webpki::Error,
    },
    /// A path verified, but holds a distrusted key.
    #[snafu(display("distrusted key at {position:?}"))]
    Distrusted {
        /// Where the key sits.
        position: PathPosition,
    },
    /// A path verified, but the revocation check refused it.
    #[snafu(display("revocation check failed"))]
    Revocation {
        /// The failure.
        source: R,
    },
    /// A path verified, but the CT check refused it.
    #[snafu(display("certificate transparency check failed"))]
    Transparency {
        /// The failure.
        source: T,
    },
    /// The certificate is not valid for the target's name.
    #[snafu(display("certificate not valid for the target"))]
    Name {
        /// webpki's failure.
        source: webpki::Error,
    },
    /// The certificate's key is not one this crate verifies with.
    #[snafu(display("end entity key unsupported"))]
    LeafKey {
        /// The failure.
        source: public_key::InvalidPublicKeyError,
    },
}

impl<R, T> VerifyCertificateChainError<R, T>
where
    R: core::error::Error + 'static,
    T: core::error::Error + 'static,
{
    /// The alert the failure calls for: decode_error for an empty
    /// Certificate (RFC 9846 §4.5.1.3); certificate_expired, unknown_ca or
    /// certificate_revoked when webpki reports that cause; unsupported
    /// certificate for a key this crate does not use; certificate_revoked
    /// for the revocation check; bad_certificate otherwise.
    pub fn alert(&self) -> AlertDescription {
        match self {
            Self::NoCertificate => AlertDescription::DECODE_ERROR,
            Self::Path { source } => match source {
                webpki::Error::CertExpired { .. } | webpki::Error::CertNotValidYet { .. } => {
                    AlertDescription::CERTIFICATE_EXPIRED
                }
                webpki::Error::UnknownIssuer => AlertDescription::UNKNOWN_CA,
                webpki::Error::CertRevoked => AlertDescription::CERTIFICATE_REVOKED,
                _ => AlertDescription::BAD_CERTIFICATE,
            },
            Self::Revocation { .. } => AlertDescription::CERTIFICATE_REVOKED,
            Self::LeafKey { .. } => AlertDescription::UNSUPPORTED_CERTIFICATE,
            Self::EndEntity { .. }
            | Self::Distrusted { .. }
            | Self::Transparency { .. }
            | Self::Name { .. } => AlertDescription::BAD_CERTIFICATE,
        }
    }
}

impl<R, T> crate::alert::CallsForAlert for VerifyCertificateChainError<R, T>
where
    R: core::error::Error + 'static,
    T: core::error::Error + 'static,
{
    fn alert(&self) -> AlertDescription {
        VerifyCertificateChainError::alert(self)
    }
}
