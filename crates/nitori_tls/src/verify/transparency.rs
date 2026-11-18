//! Certificate Transparency by the SCTs embedded in a certificate
//! (RFC 6962).
//!
//! A CA that logs a precertificate gets a signed certificate timestamp
//! (SCT) from each log and embeds them in the certificate it issues, in the
//! extension 1.3.6.1.4.1.11129.2.4.2. A log signs the precertificate's
//! TBSCertificate, which is the certificate's TBSCertificate without that
//! extension, together with the hash of the issuer's public key
//! (RFC 6962 §3.2). [`EmbeddedSctPolicy`] checks those signatures against a
//! [`LogList`] loaded at run time and counts the SCTs by the rules of
//! Chrome's CT policy.

use super::{
    der::{self, MalformedDerError, Reader},
    public_key::{InvalidPublicKeyError, PublicKeyAlgorithm, SubjectPublicKey},
};
use crate::clock::elapsed;
use crate::extension::signed_certificate_timestamp::codec::{
    ReadSignedCertificateTimestampListError, read_signed_certificate_timestamp_list,
};
use nitori_crypto::{Hash, HashAlgorithm, RunningHash, SignatureScheme, SignatureVerify};
use snafu::{ResultExt, Snafu};
use std::{
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};
use time::UtcDateTime;

/// The extension that holds the embedded SCT list,
/// 1.3.6.1.4.1.11129.2.4.2.
const SCT_LIST: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xd6, 0x79, 0x02, 0x04, 0x02];

/// The identity of a log: the SHA-256 hash of its public key's
/// SubjectPublicKeyInfo (RFC 6962 §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogId(pub [u8; 32]);

/// The state of a log in a log list, as Chrome's policy names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogState {
    /// Applied for inclusion; its SCTs do not count.
    Pending,
    /// Accepted and being monitored.
    Qualified,
    /// Trusted.
    Usable,
    /// Trusted, no longer accepting new entries.
    ReadOnly,
    /// No longer trusted for new SCTs: those with a timestamp before `at`
    /// count.
    Retired {
        /// When the log was retired.
        at: UtcDateTime,
    },
    /// Never trusted, or no longer at all; its SCTs do not count.
    Rejected,
}

impl LogState {
    /// Whether an SCT of this log with `timestamp` counts.
    fn counts(self, timestamp: UtcDateTime) -> bool {
        match self {
            Self::Qualified | Self::Usable | Self::ReadOnly => true,
            Self::Retired { at } => timestamp < at,
            Self::Pending | Self::Rejected => false,
        }
    }

    /// Whether the log is in a state that is not retired.
    fn current(self) -> bool {
        matches!(self, Self::Qualified | Self::Usable | Self::ReadOnly)
    }
}

/// One log of a [`LogList`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtLog {
    id: LogId,
    key: SubjectPublicKey,
    operator: String,
    state: LogState,
}

impl CtLog {
    /// The log with `id` and the public key of the DER SubjectPublicKeyInfo
    /// `key`, run by `operator`, in `state`.
    ///
    /// # Errors
    ///
    /// [`InvalidLogKeyError`] when `key` is not an ECDSA P-256 or RSA key, the
    /// two kinds RFC 6962 §2.1.4 allows.
    pub fn new(
        id: LogId,
        key: &[u8],
        operator: impl Into<String>,
        state: LogState,
    ) -> Result<Self, InvalidLogKeyError> {
        let key = SubjectPublicKey::from_spki(key).context(invalid_log_key::KeySnafu)?;
        if !matches!(
            key.algorithm(),
            PublicKeyAlgorithm::EcdsaP256 | PublicKeyAlgorithm::Rsa
        ) {
            return invalid_log_key::AlgorithmSnafu {
                algorithm: key.algorithm(),
            }
            .fail();
        }
        Ok(Self {
            id,
            key,
            operator: operator.into(),
            state,
        })
    }

    /// The log's identity.
    pub fn id(&self) -> LogId {
        self.id
    }

    /// The log's operator.
    pub fn operator(&self) -> &str {
        &self.operator
    }

    /// The log's state.
    pub fn state(&self) -> LogState {
        self.state
    }

    /// The signature scheme of an SCT by its `hash` and `signature`
    /// algorithms (RFC 5246 §7.4.1.4.1), when it fits the log's key.
    fn scheme(&self, hash: u8, signature: u8) -> Option<SignatureScheme> {
        match (self.key.algorithm(), hash, signature) {
            (PublicKeyAlgorithm::EcdsaP256, 4, 3) => Some(SignatureScheme::ECDSA_SECP256R1_SHA256),
            (PublicKeyAlgorithm::Rsa, 4, 1) => Some(SignatureScheme::RSA_PKCS1_SHA256),
            _ => None,
        }
    }
}

/// The key of a [`CtLog`] is not one a log may have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(module(invalid_log_key), visibility(pub(crate)))]
pub enum InvalidLogKeyError {
    /// The key is not a key this crate reads.
    #[snafu(display("log key invalid"))]
    Key {
        /// The failure.
        source: InvalidPublicKeyError,
    },
    /// The key is neither ECDSA P-256 nor RSA.
    #[snafu(display("log key of {algorithm:?}, not ECDSA P-256 or RSA"))]
    Algorithm {
        /// The key's algorithm.
        algorithm: PublicKeyAlgorithm,
    },
}

/// The logs a client trusts, as of the time the list was published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogList {
    logs: Vec<CtLog>,
    published_at: UtcDateTime,
}

impl LogList {
    /// The list of `logs`, published at `published_at`.
    pub fn new(logs: Vec<CtLog>, published_at: UtcDateTime) -> Self {
        Self { logs, published_at }
    }

    /// The logs.
    pub fn logs(&self) -> &[CtLog] {
        &self.logs
    }

    /// When the list was published.
    pub fn published_at(&self) -> UtcDateTime {
        self.published_at
    }

    fn find(&self, id: &[u8; 32]) -> Option<&CtLog> {
        self.logs.iter().find(|log| &log.id.0 == id)
    }
}

/// What [`EmbeddedSctPolicy`] does once its log list is older than its
/// maximum age.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StaleLogListAction {
    /// Every certificate fails: the check fails closed.
    Reject,
    /// Certificates are not checked for SCTs: the check fails open, as
    /// Chrome does.
    SkipCheck,
}

/// CT by the SCTs embedded in a certificate, counted by the rules of
/// Chrome's CT policy for embedded SCTs.
///
/// An SCT counts when its log is in the list, its signature verifies with
/// the log's key, its timestamp is not in the future, and its log is
/// Qualified, Usable or ReadOnly, or Retired after the timestamp. Each log
/// counts once. A certificate whose validity is at most 180 days needs 2
/// such SCTs, a longer one 3; at least 2 of them come from different
/// operators, and at least 1 from a log that is not Retired.
///
/// SCTs sent in the handshake or in an OCSP response are not considered.
///
/// The list can be replaced while the policy is in use. Once it is older
/// than the maximum age, the policy acts by its [`StaleLogListAction`]: by
/// default it rejects every certificate, and the maximum age is
/// [`EmbeddedSctPolicy::CHROME_MAX_LIST_AGE`].
#[derive(Debug)]
pub struct EmbeddedSctPolicy {
    list: RwLock<Arc<LogList>>,
    max_list_age: Duration,
    when_stale: StaleLogListAction,
}

impl EmbeddedSctPolicy {
    /// The age past which Chrome stops checking CT: 70 days.
    pub const CHROME_MAX_LIST_AGE: Duration = Duration::from_secs(70 * 86_400);

    /// The validity up to which a certificate needs 2 SCTs: 180 days.
    const SHORT_VALIDITY: Duration = Duration::from_secs(180 * 86_400);

    /// The policy over `list`, which rejects every certificate once the
    /// list is older than [`EmbeddedSctPolicy::CHROME_MAX_LIST_AGE`].
    pub fn new(list: LogList) -> Self {
        Self::with_list_age(list, Self::CHROME_MAX_LIST_AGE, StaleLogListAction::Reject)
    }

    /// The policy over `list`, which acts by `when_stale` once the list is
    /// older than `max_list_age`.
    pub fn with_list_age(
        list: LogList,
        max_list_age: Duration,
        when_stale: StaleLogListAction,
    ) -> Self {
        Self {
            list: RwLock::new(Arc::new(list)),
            max_list_age,
            when_stale,
        }
    }

    /// Replaces the log list; checks in progress keep the list they began
    /// with.
    pub fn replace_list(&self, list: LogList) {
        *self.list.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(list);
    }

    fn list(&self) -> Arc<LogList> {
        Arc::clone(&self.list.read().unwrap_or_else(PoisonError::into_inner))
    }
}

impl<C: Hash + SignatureVerify> super::Transparency<C> for EmbeddedSctPolicy {
    type Error = TransparencyError;

    fn check(
        &self,
        backend: &C,
        leaf: &[u8],
        issuer_spki: &[u8],
        now: UtcDateTime,
    ) -> Result<(), TransparencyError> {
        use transparency_error::*;
        let list = self.list();
        let age = elapsed(list.published_at, now).unwrap_or_default();
        if age > self.max_list_age {
            return match self.when_stale {
                StaleLogListAction::Reject => StaleLogListSnafu { age }.fail(),
                StaleLogListAction::SkipCheck => Ok(()),
            };
        }
        let sha256 = backend
            .hash_algorithm(HashAlgorithm::Sha256)
            .ok_or(TransparencyError::Sha256Unsupported)?;
        let leaf = Leaf::parse(leaf).context(MalformedCertificateSnafu)?;
        let mut issuer_key_hash = Hash::start(backend, sha256);
        issuer_key_hash.update(issuer_spki);
        let issuer_key_hash = issuer_key_hash.finish();
        let mut counted: Vec<&CtLog> = Vec::new();
        if let Some(list_bytes) = leaf.timestamps {
            let timestamps = read_signed_certificate_timestamp_list(&mut { list_bytes })
                .context(MalformedTimestampListSnafu)?;
            for serialized in timestamps.timestamps() {
                let Some(sct) = Sct::parse(serialized) else {
                    continue;
                };
                let Some(log) = list.find(&sct.log_id) else {
                    continue;
                };
                // A timestamp past the years a date can hold is in the future.
                let Ok(timestamp) =
                    UtcDateTime::from_unix_timestamp_nanos(i128::from(sct.timestamp) * 1_000_000)
                else {
                    continue;
                };
                if timestamp > now
                    || !log.state.counts(timestamp)
                    || counted.iter().any(|seen| seen.id == log.id)
                {
                    continue;
                }
                let Some(scheme) = log
                    .scheme(sct.hash, sct.signature_algorithm)
                    .and_then(|scheme| backend.signature_scheme(scheme))
                else {
                    continue;
                };
                let signed = sct.signed_data(issuer_key_hash.as_bytes(), &leaf.precert_tbs);
                if SignatureVerify::verify(backend, scheme, log.key.key(), &signed, sct.signature)
                    .is_ok()
                {
                    counted.push(log);
                }
            }
        }
        let validity = elapsed(leaf.not_before, leaf.not_after).unwrap_or_default();
        let required = if validity <= Self::SHORT_VALIDITY {
            2
        } else {
            3
        };
        if counted.len() < required {
            return NotEnoughTimestampsSnafu {
                valid: counted.len(),
                required,
            }
            .fail();
        }
        let mut operators: Vec<&str> = counted.iter().map(|log| log.operator()).collect();
        operators.sort_unstable();
        operators.dedup();
        if operators.len() < 2 {
            return TooFewOperatorsSnafu.fail();
        }
        if !counted.iter().any(|log| log.state.current()) {
            return NoCurrentLogSnafu.fail();
        }
        Ok(())
    }
}

/// A certificate does not meet [`EmbeddedSctPolicy`].
#[derive(Debug, Snafu)]
#[snafu(module(transparency_error), visibility(pub(crate)))]
pub enum TransparencyError {
    /// The log list is older than the policy's maximum age, and the policy
    /// rejects then.
    #[snafu(display("log list is {} s old", age.as_secs()))]
    StaleLogList {
        /// The age of the list.
        age: Duration,
    },
    /// The backend does not hash with SHA-256, which logs use.
    #[snafu(display("backend does not support SHA-256"))]
    Sha256Unsupported,
    /// The certificate's structure could not be read.
    #[snafu(display("certificate malformed"))]
    MalformedCertificate {
        /// The failure.
        source: MalformedDerError,
    },
    /// The embedded SCT list could not be read.
    #[snafu(display("embedded SCT list malformed"))]
    MalformedTimestampList {
        /// The failure.
        source: ReadSignedCertificateTimestampListError,
    },
    /// Fewer SCTs count than the certificate's validity calls for.
    #[snafu(display("{valid} valid SCTs where {required} are required"))]
    NotEnoughTimestamps {
        /// The number of SCTs that count, below `required`.
        valid: usize,
        /// The number required: 2 or 3.
        required: usize,
    },
    /// The SCTs that count all come from one operator.
    #[snafu(display("valid SCTs come from a single log operator"))]
    TooFewOperators,
    /// The SCTs that count all come from retired logs.
    #[snafu(display("no valid SCT from a log that is not retired"))]
    NoCurrentLog,
}

/// What the policy reads of a certificate.
struct Leaf<'a> {
    /// The precertificate's TBSCertificate: the certificate's, without the
    /// SCT list extension.
    precert_tbs: Vec<u8>,
    /// The TLS encoding of the embedded SCT list, if there is one.
    timestamps: Option<&'a [u8]>,
    not_before: UtcDateTime,
    not_after: UtcDateTime,
}

impl<'a> Leaf<'a> {
    fn parse(certificate: &'a [u8]) -> Result<Self, MalformedDerError> {
        let mut parts = Reader::new(Reader::single(certificate, der::SEQUENCE)?);
        let tbs = parts.expect(der::SEQUENCE)?.contents;
        let mut fields = Reader::new(tbs);
        fields.optional(0xa0)?;
        fields.expect(der::INTEGER)?;
        fields.expect(der::SEQUENCE)?;
        fields.expect(der::SEQUENCE)?;
        let mut validity = Reader::new(fields.expect(der::SEQUENCE)?.contents);
        let not_before = der::read_time(validity.read()?)?;
        let not_after = der::read_time(validity.read()?)?;
        if !validity.is_empty() {
            return Err(MalformedDerError);
        }
        fields.expect(der::SEQUENCE)?;
        fields.expect(der::SEQUENCE)?;
        fields.optional(0x81)?;
        fields.optional(0x82)?;
        let before_extensions = &tbs[..tbs.len() - fields.rest().len()];
        let extensions = fields.optional(0xa3)?;
        if !fields.is_empty() {
            return Err(MalformedDerError);
        }
        let mut kept = Vec::new();
        let mut timestamps = None;
        if let Some(extensions) = extensions {
            let mut items = Reader::new(Reader::single(extensions.contents, der::SEQUENCE)?);
            while !items.is_empty() {
                let item = items.expect(der::SEQUENCE)?;
                let mut extension = Reader::new(item.contents);
                if extension.expect(der::OBJECT_IDENTIFIER)?.contents != SCT_LIST {
                    kept.extend_from_slice(item.encoded);
                    continue;
                }
                extension.optional(der::BOOLEAN)?;
                let value = extension.expect(der::OCTET_STRING)?.contents;
                if !extension.is_empty() || timestamps.is_some() {
                    return Err(MalformedDerError);
                }
                timestamps = Some(Reader::single(value, der::OCTET_STRING)?);
            }
        }
        let mut contents = before_extensions.to_vec();
        if !kept.is_empty() {
            contents.extend_from_slice(&der::wrap(0xa3, &der::wrap(der::SEQUENCE, &kept)));
        }
        Ok(Self {
            precert_tbs: der::wrap(der::SEQUENCE, &contents),
            timestamps,
            not_before,
            not_after,
        })
    }
}

/// An SCT of version 1 (RFC 6962 §3.2).
struct Sct<'a> {
    log_id: [u8; 32],
    timestamp: u64,
    extensions: &'a [u8],
    hash: u8,
    signature_algorithm: u8,
    signature: &'a [u8],
}

impl<'a> Sct<'a> {
    /// The SCT of `bytes`, or `None` when it is of another version or
    /// malformed; such an SCT does not count.
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        let (&version, rest) = bytes.split_first()?;
        if version != 0 {
            return None;
        }
        let (log_id, rest) = rest.split_first_chunk::<32>()?;
        let (timestamp, rest) = rest.split_first_chunk::<8>()?;
        let (extensions, rest) = vector(rest)?;
        let (&[hash, signature_algorithm], rest) = rest.split_first_chunk::<2>()?;
        let (signature, rest) = vector(rest)?;
        if !rest.is_empty() {
            return None;
        }
        Some(Self {
            log_id: *log_id,
            timestamp: u64::from_be_bytes(*timestamp),
            extensions,
            hash,
            signature_algorithm,
            signature,
        })
    }

    /// What the log signed for a precertificate entry with `issuer_key_hash`
    /// and the precertificate's TBSCertificate `tbs`.
    fn signed_data(&self, issuer_key_hash: &[u8], tbs: &[u8]) -> Vec<u8> {
        let mut data = Vec::with_capacity(12 + 32 + 3 + tbs.len() + 2 + self.extensions.len());
        // sct_version v1, signature_type certificate_timestamp.
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&self.timestamp.to_be_bytes());
        // entry_type precert_entry.
        data.extend_from_slice(&[0, 1]);
        data.extend_from_slice(issuer_key_hash);
        // invariant: a certificate in a handshake message is below 2^24
        // bytes, and so is its TBSCertificate.
        data.extend_from_slice(&(tbs.len() as u32).to_be_bytes()[1..]);
        data.extend_from_slice(tbs);
        // invariant: the extensions were read with a two-byte length.
        data.extend_from_slice(&(self.extensions.len() as u16).to_be_bytes());
        data.extend_from_slice(self.extensions);
        data
    }
}

/// A field with a two-byte length, and what follows it.
fn vector(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, rest) = bytes.split_first_chunk::<2>()?;
    let len = usize::from(u16::from_be_bytes(*len));
    (rest.len() >= len).then(|| rest.split_at(len))
}
