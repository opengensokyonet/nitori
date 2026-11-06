use aws_lc_rs::kem::{Ciphertext, DecapsulationKey, EncapsulationKey, ML_KEM_768};
use nitori_crypto::{
    CompleteError, DecapsulationFailedSnafu, InvalidPeerShareSnafu, KeyGenerationFailedError,
    KeyGenerationFailedSnafu, KeyShare, NamedGroup, Secret, ServerShareError,
};
use snafu::{OptionExt, ensure};

/// The length of an ML-KEM-768 encapsulation key (FIPS 203 §8).
pub(super) const ENCAPSULATION_KEY_LEN: usize = 1184;
/// The length of an ML-KEM-768 ciphertext (FIPS 203 §8).
pub(super) const CIPHERTEXT_LEN: usize = 1088;

/// The decapsulation key of an ML-KEM-768 exchange.
pub(crate) struct MlKem768 {
    key: DecapsulationKey,
}

impl MlKem768 {
    /// A new decapsulation key and the share that carries its encapsulation
    /// key.
    pub(super) fn generate(
        group: NamedGroup,
    ) -> Result<(Self, KeyShare), KeyGenerationFailedError> {
        let key = DecapsulationKey::generate(&ML_KEM_768)
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        let share = key
            .encapsulation_key()
            .and_then(|encapsulation| encapsulation.key_bytes())
            .ok()
            .context(KeyGenerationFailedSnafu { group })?;
        Ok((Self { key }, KeyShare::from_slice(share.as_ref())))
    }

    /// The ML-KEM shared secret of `ciphertext`.
    pub(super) fn decapsulate(
        &self,
        group: NamedGroup,
        ciphertext: &[u8],
    ) -> Result<aws_lc_rs::kem::SharedSecret, CompleteError> {
        // RFC 10024 §4.2: a ciphertext of the wrong length is an invalid
        // share; any other decapsulation failure is the client's own.
        ensure!(
            ciphertext.len() == CIPHERTEXT_LEN,
            InvalidPeerShareSnafu { group }
        );
        Ok(self
            .key
            .decapsulate(Ciphertext::from(ciphertext))
            .ok()
            .context(DecapsulationFailedSnafu { group })?)
    }

    pub(super) fn complete(
        self,
        group: NamedGroup,
        server_share: &[u8],
    ) -> Result<Secret, CompleteError> {
        let shared = self.decapsulate(group, server_share)?;
        // An ML-KEM shared secret is 32 bytes.
        Ok(Secret::new(shared.as_ref()).expect("shared secret fits in a secret"))
    }
}

/// Encapsulate to `encapsulation_key`: the ciphertext and the shared
/// secret.
pub(super) fn encapsulate(
    group: NamedGroup,
    encapsulation_key: &[u8],
) -> Result<(Ciphertext<'static>, aws_lc_rs::kem::SharedSecret), ServerShareError> {
    let key = EncapsulationKey::new(&ML_KEM_768, encapsulation_key)
        .ok()
        .context(InvalidPeerShareSnafu { group })?;
    // Encapsulation performs the encapsulation key check of FIPS 203 §7.2
    // and fails when the key does not pass it. aws-lc-rs reports that
    // failure and an internal one alike; both are reported as an invalid
    // share.
    Ok(key
        .encapsulate()
        .ok()
        .context(InvalidPeerShareSnafu { group })?)
}

/// The server's step of ML-KEM-768: the ciphertext as its share, and the
/// shared secret.
pub(super) fn server_share(
    group: NamedGroup,
    client_share: &[u8],
) -> Result<(KeyShare, Secret), ServerShareError> {
    let (ciphertext, shared) = encapsulate(group, client_share)?;
    // An ML-KEM shared secret is 32 bytes.
    let secret = Secret::new(shared.as_ref()).expect("shared secret fits in a secret");
    Ok((KeyShare::from_slice(ciphertext.as_ref()), secret))
}
