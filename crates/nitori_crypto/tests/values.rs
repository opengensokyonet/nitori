use nitori_crypto::{
    Digest, ExpandLength, HashAlgorithm, Issuer, KeyShare, NamedGroup, Secret, Signature,
    SignatureScheme, Supported,
};

struct Backend;

mod token {
    pub struct Token(());

    impl Token {
        pub(crate) fn new() -> Self {
            Self(())
        }
    }
}

impl Issuer for Backend {
    type Token = token::Token;
}

/// A type that is neither `Send` nor `Sync`.
struct Local(core::marker::PhantomData<*const ()>);

impl Issuer for Local {
    type Token = ();
}

const _: () = {
    const fn shared<T: Send + Sync + Copy>() {}
    shared::<Supported<Backend, NamedGroup>>();
    shared::<Supported<Local, HashAlgorithm>>();
};

#[test]
fn supported_holds_the_algorithm() {
    let supported = Supported::<Backend, _>::new(token::Token::new(), NamedGroup::X25519);
    assert_eq!(supported.algorithm(), NamedGroup::X25519);
    assert_eq!(supported, supported.clone());
    assert_eq!(format!("{supported:?}"), "Supported(NamedGroup(x25519))");
    assert_eq!(core::mem::size_of_val(&supported), 2);
}

#[test]
fn expand_length_is_checked() {
    assert_eq!(ExpandLength::new(0).unwrap_err().requested_len(), 0);
    assert_eq!(ExpandLength::new(65).unwrap_err().requested_len(), 65);
    assert_eq!(ExpandLength::new(1).unwrap(), ExpandLength::MIN);
    assert_eq!(ExpandLength::new(64).unwrap(), ExpandLength::MAX);
    for algorithm in [HashAlgorithm::Sha256, HashAlgorithm::Sha384] {
        assert_eq!(
            usize::from(ExpandLength::hash_len(algorithm).get()),
            algorithm.output_len()
        );
    }
}

#[test]
fn secrets_are_bounded_and_hidden() {
    let error = Secret::new(&[0; Secret::CAPACITY + 1]).unwrap_err();
    assert_eq!(error.requested_len(), Secret::CAPACITY + 1);
    let error = Secret::from_parts(&[&[0; 32], &[0; 35]]).unwrap_err();
    assert_eq!(error.requested_len(), 67);

    let secret = Secret::from_parts(&[&[1; 32], &[2; 34]]).unwrap();
    assert_eq!(secret.len(), Secret::CAPACITY);
    assert_eq!(&secret.as_bytes()[31..33], [1, 2]);
    assert_eq!(format!("{secret:?}"), "Secret { len: 66, .. }");

    let mut zeroed = Secret::zeroed(ExpandLength::new(12).unwrap());
    assert_eq!(zeroed.as_bytes(), [0; 12]);
    zeroed.as_mut_bytes()[0] = 7;
    assert_eq!(zeroed.as_bytes()[0], 7);
    assert_eq!(zeroed.len(), 12);
    assert!(Secret::new(&[]).unwrap().is_empty());
}

#[test]
fn digest_length_matches_the_algorithm() {
    let error = Digest::new(HashAlgorithm::Sha384, &[0; 32]).unwrap_err();
    assert_eq!(error.algorithm(), HashAlgorithm::Sha384);
    assert_eq!(error.requested_len(), 32);
    let digest = Digest::new(HashAlgorithm::Sha256, &[0xab; 32]).unwrap();
    assert_eq!(digest.as_bytes(), [0xab; 32]);
    assert!(format!("{digest:?}").starts_with("Digest(abab"));
}

#[test]
fn shares_and_signatures_hold_any_length() {
    let mut share = KeyShare::from_slice(&[1; 97]);
    share.extend_from_slice(&[2; 1119]);
    assert_eq!(share.as_bytes().len(), 1216);
    assert_eq!(share.as_bytes()[96..98], [1, 2]);
    let signature = Signature::from_slice(&[3; 512]);
    assert_eq!(signature.as_bytes(), [3; 512]);
}

#[test]
fn code_points_print_their_names() {
    assert_eq!(
        format!("{:?}", NamedGroup::X25519MLKEM768),
        "NamedGroup(X25519MLKEM768)"
    );
    assert_eq!(
        format!("{:?}", NamedGroup::new(0x1234)),
        "NamedGroup(0x1234)"
    );
    assert_eq!(NamedGroup::X25519MLKEM768.get(), 4588);
    assert_eq!(
        format!("{:?}", SignatureScheme::RSA_PSS_RSAE_SHA256),
        "SignatureScheme(rsa_pss_rsae_sha256)"
    );
    assert_eq!(SignatureScheme::new(0x0807), SignatureScheme::ED25519);
}
