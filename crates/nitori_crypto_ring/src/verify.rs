use nitori_crypto::{BadSignatureError, SignatureScheme, SignatureVerify, Supported};
use ring::signature;

use crate::{RingBackend, token::Token};

const SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::ECDSA_SECP256R1_SHA256,
    SignatureScheme::ECDSA_SECP384R1_SHA384,
    SignatureScheme::ED25519,
    SignatureScheme::RSA_PSS_RSAE_SHA256,
    SignatureScheme::RSA_PSS_RSAE_SHA384,
    SignatureScheme::RSA_PSS_RSAE_SHA512,
    SignatureScheme::RSA_PKCS1_SHA256,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA512,
];

fn verification_algorithm(
    scheme: SignatureScheme,
) -> Option<&'static dyn signature::VerificationAlgorithm> {
    Some(match scheme {
        SignatureScheme::ECDSA_SECP256R1_SHA256 => &signature::ECDSA_P256_SHA256_ASN1,
        SignatureScheme::ECDSA_SECP384R1_SHA384 => &signature::ECDSA_P384_SHA384_ASN1,
        SignatureScheme::ED25519 => &signature::ED25519,
        SignatureScheme::RSA_PSS_RSAE_SHA256 => &signature::RSA_PSS_2048_8192_SHA256,
        SignatureScheme::RSA_PSS_RSAE_SHA384 => &signature::RSA_PSS_2048_8192_SHA384,
        SignatureScheme::RSA_PSS_RSAE_SHA512 => &signature::RSA_PSS_2048_8192_SHA512,
        SignatureScheme::RSA_PKCS1_SHA256 => &signature::RSA_PKCS1_2048_8192_SHA256,
        SignatureScheme::RSA_PKCS1_SHA384 => &signature::RSA_PKCS1_2048_8192_SHA384,
        SignatureScheme::RSA_PKCS1_SHA512 => &signature::RSA_PKCS1_2048_8192_SHA512,
        _ => return None,
    })
}

impl SignatureVerify for RingBackend {
    fn signature_schemes(&self) -> &[SignatureScheme] {
        SCHEMES
    }

    fn signature_scheme(
        &self,
        scheme: SignatureScheme,
    ) -> Option<Supported<Self, SignatureScheme>> {
        verification_algorithm(scheme).map(|_| Supported::new(Token::new(), scheme))
    }

    fn verify(
        &self,
        scheme: Supported<Self, SignatureScheme>,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), BadSignatureError> {
        // `RingBackend` issues `Supported` only for schemes that have an
        // algorithm here.
        let algorithm =
            verification_algorithm(scheme.algorithm()).expect("supported scheme has an algorithm");
        signature::UnparsedPublicKey::new(algorithm, public_key)
            .verify(message, signature)
            .map_err(|_| BadSignatureError)
    }
}
