use aws_lc_rs::signature;
use nitori_crypto::{BadSignatureError, SignatureScheme, SignatureVerify, Supported};

use crate::{AwsLcBackend, token::Token};

const SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::ECDSA_SECP256R1_SHA256,
    SignatureScheme::ECDSA_SECP384R1_SHA384,
    SignatureScheme::ECDSA_SECP521R1_SHA512,
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
        SignatureScheme::ECDSA_SECP521R1_SHA512 => &signature::ECDSA_P521_SHA512_ASN1,
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

/// What the `subjectPublicKey` bit string of a key for a scheme holds.
#[derive(Clone, Copy)]
enum KeyForm {
    /// An uncompressed point of this length (SEC 1 §2.3.3).
    Point(usize),
    /// The 32 bytes of an Ed25519 key (RFC 8410 §4).
    Ed25519,
    /// A DER RSAPublicKey (RFC 8017 §A.1.1).
    RsaPublicKey,
}

fn key_form(scheme: SignatureScheme) -> KeyForm {
    match scheme {
        SignatureScheme::ECDSA_SECP256R1_SHA256 => KeyForm::Point(65),
        SignatureScheme::ECDSA_SECP384R1_SHA384 => KeyForm::Point(97),
        SignatureScheme::ECDSA_SECP521R1_SHA512 => KeyForm::Point(133),
        SignatureScheme::ED25519 => KeyForm::Ed25519,
        SignatureScheme::RSA_PSS_RSAE_SHA256
        | SignatureScheme::RSA_PSS_RSAE_SHA384
        | SignatureScheme::RSA_PSS_RSAE_SHA512
        | SignatureScheme::RSA_PKCS1_SHA256
        | SignatureScheme::RSA_PKCS1_SHA384
        | SignatureScheme::RSA_PKCS1_SHA512 => KeyForm::RsaPublicKey,
        // invariant: `verify` takes only the schemes in `SCHEMES`.
        _ => unreachable!("{scheme:?} is not a supported scheme"),
    }
}

/// Whether `public_key` has `form`, as far as aws-lc-rs would otherwise
/// accept more.
///
/// aws-lc-rs also parses a whole SubjectPublicKeyInfo for every algorithm,
/// and compressed and hybrid points for ECDSA; the trait accepts neither.
fn has_form(form: KeyForm, public_key: &[u8]) -> bool {
    match form {
        KeyForm::Point(len) => public_key.len() == len && public_key.first() == Some(&0x04),
        KeyForm::Ed25519 => public_key.len() == 32,
        KeyForm::RsaPublicKey => starts_with_integer(public_key),
    }
}

/// Whether the first element of the DER SEQUENCE `der` is an INTEGER.
///
/// An RSAPublicKey starts with the modulus, a SubjectPublicKeyInfo with
/// the AlgorithmIdentifier SEQUENCE. aws-lc-rs tries an RSAPublicKey first
/// and checks the rest itself.
fn starts_with_integer(der: &[u8]) -> bool {
    const SEQUENCE: u8 = 0x30;
    const INTEGER: u8 = 0x02;
    let [SEQUENCE, length, rest @ ..] = der else {
        return false;
    };
    // A short-form length is the length itself; a long-form one gives the
    // number of length bytes that follow.
    let skip = if length & 0x80 == 0 {
        0
    } else {
        usize::from(length & 0x7f)
    };
    rest.get(skip) == Some(&INTEGER)
}

impl SignatureVerify for AwsLcBackend {
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
        // `AwsLcBackend` issues `Supported` only for schemes that have an
        // algorithm here.
        let algorithm =
            verification_algorithm(scheme.algorithm()).expect("supported scheme has an algorithm");
        if !has_form(key_form(scheme.algorithm()), public_key) {
            return Err(BadSignatureError);
        }
        signature::UnparsedPublicKey::new(algorithm, public_key)
            .verify(message, signature)
            .map_err(|_| BadSignatureError)
    }
}
