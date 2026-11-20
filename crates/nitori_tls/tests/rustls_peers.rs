//! The handshake engines against rustls's QUIC connections, in memory: the
//! server's engine against `rustls::quic::ClientConnection`, the client's
//! engine against `rustls::quic::ServerConnection`.
//!
//! What one side writes at a level moves to the other side at the same
//! level. rustls logs its secrets through a `KeyLog`; the tests check that
//! the engine hands over the same traffic secrets and logs the same secrets,
//! with the same client_random, through its own key log, that both exporters
//! export the same bytes, and that both sides agree on the application
//! protocol and on each other's transport parameters. The server presents
//! the Ed25519 certificate for example.com of `common::pki`, which both
//! clients trust through its root.
#![feature(impl_trait_in_assoc_type, yield_expr)]

mod common;

use bytes::Bytes;
use common::{
    now,
    pki::{self, leaf_key, verifier},
    steps::{self, ClientSteps, ServerSteps},
    streams::{
        ClientParameters, ClientSide, Incoming, LEVELS, Outgoing, ServerParameters, ServerSide,
        Side, index,
    },
};
use core::{
    fmt::Debug,
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{LentCall, Receiver};
use nitori_crypto::AeadAlgorithm;
use nitori_crypto::{ExpandLength, NamedGroup};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_sync::MutexSynchronization;
use nitori_tls::schedule::TrafficSecret;
use nitori_tls::{
    cipher_suite::CipherSuite,
    client::{
        ServerTarget,
        engine::{
            ClientHandshake, ClientHandshakeCompleted, ClientHandshakeSettings, EarlyDataOffered,
            EarlyDataOutcome, ServerHelloRead, ServerIdentity,
        },
        identity::NoClientCertificate,
        offering::PreferenceOffering,
        ticket::TakeOnceTicketMap,
    },
    clock::ManualClock,
    engine::CryptoLevel,
    extension::alpn::{ProtocolName, ProtocolNameList},
    key_log::{KeyLog as EngineKeyLog, KeyLogLabel},
    message::{HelloRandom, MessageLimits, new_session_ticket::TicketLifetime},
    schedule::ExporterLabel,
    server::{
        choice::{GroupPreference, PreferenceTable},
        engine::{
            ServerHandshake, ServerHandshakeCompleted, ServerHandshakeSettings, ServerHelloWritten,
        },
        identity::CertifiedKey,
        layers::PassThroughLayers,
        peer::NoCertificateRequest,
        targets::SniTargets,
        ticket::ZeroRttOnceTicketTable,
    },
};
use rustls::{
    ClientConfig, HandshakeKind, KeyLog, RootCertStore, ServerConfig,
    crypto::{
        CryptoProvider,
        aws_lc_rs::{default_provider, kx_group},
    },
    pki_types::{CertificateDer, PrivateKeyDer, ServerName},
    quic::{self, KeyChange, Version},
};
use std::{
    collections::HashMap,
    hash::RandomState,
    sync::{Arc, Mutex},
    time::Duration,
};

const SERVER_PARAMETERS: &[u8] = b"server parameters";
const CLIENT_PARAMETERS: &[u8] = b"client parameters";
const EXPORTER_LABEL: &[u8] = b"EXPORTER-test";

fn h3() -> ProtocolName<Bytes> {
    ProtocolName::new(Bytes::from_static(b"h3")).unwrap()
}

/// The secrets a rustls side logs, taken one connection at a time.
#[derive(Debug, Default)]
struct Secrets(Mutex<Vec<(String, LoggedSecret)>>);

/// The client_random and the secret of one logged line.
type LoggedSecret = (Vec<u8>, Vec<u8>);

impl KeyLog for Secrets {
    fn log(&self, label: &str, client_random: &[u8], secret: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .push((label.to_owned(), (client_random.to_vec(), secret.to_vec())));
    }
}

/// The engine under test logs into the same kind of record.
impl EngineKeyLog for Secrets {
    fn log(&self, label: KeyLogLabel, client_random: &HelloRandom, secret: &[u8]) {
        KeyLog::log(self, label.as_str(), &client_random.0, secret);
    }
}

impl Secrets {
    /// The secrets logged since the last call, by label.
    fn take(&self) -> Logged {
        Logged(
            core::mem::take(&mut *self.0.lock().unwrap())
                .into_iter()
                .collect(),
        )
    }
}

/// The secrets logged for one connection, by label.
struct Logged(HashMap<String, LoggedSecret>);

impl Logged {
    /// Checks that `secret` is the one logged as `label`.
    fn check(&self, label: &str, secret: &TrafficSecret) {
        let (_, logged) = self.0.get(label).unwrap_or_else(|| panic!("no {label}"));
        assert_eq!(secret.as_bytes(), logged.as_slice(), "{label}");
    }

    /// Checks that the engine logged the five secrets of every handshake,
    /// and the early one exactly when `early`, each as rustls logged it,
    /// with the same client_random.
    fn check_engine(&self, rustls: &Logged, early: bool) {
        let mut labels = vec![
            "CLIENT_HANDSHAKE_TRAFFIC_SECRET",
            "SERVER_HANDSHAKE_TRAFFIC_SECRET",
            "CLIENT_TRAFFIC_SECRET_0",
            "SERVER_TRAFFIC_SECRET_0",
            "EXPORTER_SECRET",
        ];
        if early {
            labels.push("CLIENT_EARLY_TRAFFIC_SECRET");
        }
        assert_eq!(self.0.len(), labels.len(), "{:?}", self.0.keys());
        for label in labels {
            let engine = self.0.get(label).unwrap_or_else(|| panic!("no {label}"));
            assert_eq!(Some(engine), rustls.0.get(label), "{label}");
        }
    }
}

/// A rustls QUIC connection and the level it writes at.
struct Peer {
    connection: quic::Connection,
    writing: CryptoLevel,
    error: Option<rustls::Error>,
}

impl Peer {
    fn new(connection: impl Into<quic::Connection>) -> Self {
        Self {
            connection: connection.into(),
            writing: CryptoLevel::Initial,
            error: None,
        }
    }

    /// Gives rustls what the engine wrote, level by level, and moves what
    /// rustls writes to the engine's input; whether anything moved.
    ///
    /// What rustls writes is taken after each level it reads: write_hs
    /// hands out a key change only after the bytes queued before it, so
    /// bytes queued after reading the next level would come out at the
    /// old one.
    fn exchange(&mut self, from: &mut Outgoing, into: &mut Incoming) -> bool {
        let mut moved = self.write(into);
        for level in LEVELS {
            let bytes = core::mem::take(&mut from.levels[index(level)]);
            if bytes.is_empty() {
                continue;
            }
            moved = true;
            if self.error.is_none()
                && let Err(error) = self.connection.read_hs(&bytes)
            {
                self.error = Some(error);
            }
            self.write(into);
        }
        moved
    }

    /// Moves what rustls writes to `into`, each part at its level; whether
    /// anything moved.
    fn write(&mut self, into: &mut Incoming) -> bool {
        let mut moved = false;
        loop {
            let mut bytes = Vec::new();
            let change = self.connection.write_hs(&mut bytes);
            moved |= !bytes.is_empty();
            into.levels[index(self.writing)].extend(bytes);
            self.writing = match change {
                None => break,
                Some(KeyChange::Handshake { .. }) => CryptoLevel::Handshake,
                Some(KeyChange::OneRtt { .. }) => CryptoLevel::OneRtt,
            };
            moved = true;
        }
        moved
    }

    fn client(&self) -> &quic::ClientConnection {
        match &self.connection {
            quic::Connection::Client(client) => client,
            quic::Connection::Server(_) => panic!("a rustls server"),
        }
    }

    /// Checks what both sides must agree on once the handshake is over:
    /// the exporter, the application protocol and the peer's transport
    /// parameters.
    fn check_agreed(
        &self,
        export: impl Fn(&[u8]) -> Vec<u8>,
        application_protocol: &Option<ProtocolName<Bytes>>,
        peer_parameters: &[u8],
    ) {
        assert!(self.error.is_none(), "rustls failed: {:?}", self.error);
        assert!(!self.connection.is_handshaking());
        for context in [&b"context"[..], b"other"] {
            let exported = self
                .connection
                .export_keying_material([0; 32], EXPORTER_LABEL, Some(context))
                .unwrap();
            assert_eq!(export(context), exported);
        }
        assert_eq!(self.connection.alpn_protocol(), Some(&b"h3"[..]));
        assert_eq!(application_protocol, &Some(h3()));
        assert_eq!(
            self.connection.negotiated_cipher_suite().unwrap().suite(),
            rustls::CipherSuite::TLS13_AES_128_GCM_SHA256
        );
        let expected_peer = match self.connection {
            quic::Connection::Client(_) => CLIENT_PARAMETERS,
            quic::Connection::Server(_) => SERVER_PARAMETERS,
        };
        assert_eq!(peer_parameters, expected_peer);
        let own = match self.connection {
            quic::Connection::Client(_) => SERVER_PARAMETERS,
            quic::Connection::Server(_) => CLIENT_PARAMETERS,
        };
        assert_eq!(self.connection.quic_transport_parameters(), Some(own));
    }
}

/// Polls the engine until it waits, then lets `peer` take and give bytes,
/// until neither moves; the events and how the engine ended, if it did.
fn run<E, F, P>(
    peer: &mut Peer,
    side: &mut Side<P>,
    mut poll: impl FnMut(&mut Side<P>) -> Poll<ControlFlow<F, E>>,
) -> (Vec<E>, Option<F>) {
    let mut events = Vec::new();
    let mut end = None;
    for _ in 0..64 {
        let mut progress = false;
        while end.is_none() {
            match poll(side) {
                Poll::Ready(ControlFlow::Continue(event)) => events.push(event),
                Poll::Ready(ControlFlow::Break(done)) => end = Some(done),
                Poll::Pending => break,
            }
            progress = true;
        }
        progress |= peer.exchange(&mut side.output, &mut side.input);
        if !progress {
            return (events, end);
        }
    }
    panic!("the handshake does not settle");
}

/// The provider of a rustls side, with `kx_groups` when given.
fn provider(
    kx_groups: Option<Vec<&'static dyn rustls::crypto::SupportedKxGroup>>,
) -> Arc<CryptoProvider> {
    let mut provider = default_provider();
    if let Some(kx_groups) = kx_groups {
        provider.kx_groups = kx_groups;
    }
    Arc::new(provider)
}

mod server_engine {
    //! The server's engine against a rustls client.

    use super::*;

    type Event = steps::ServerEvent<AwsLcBackend, &'static str>;

    /// What the server keeps across connections.
    struct Server {
        clock: Arc<ManualClock<MutexSynchronization>>,
        tickets: ZeroRttOnceTicketTable<
            &'static str,
            AwsLcBackend,
            Arc<ManualClock<MutexSynchronization>>,
            MutexSynchronization,
            RandomState,
        >,
        groups: Vec<NamedGroup>,
        group_preference: GroupPreference,
    }

    impl Server {
        fn new() -> Self {
            let clock = Arc::new(ManualClock::new(now()));
            Self {
                tickets: ZeroRttOnceTicketTable::new(
                    AwsLcBackend,
                    Arc::clone(&clock),
                    2,
                    TicketLifetime::new(3600).unwrap(),
                    true,
                    16,
                ),
                groups: vec![NamedGroup::X25519, NamedGroup::SECP256R1],
                group_preference: GroupPreference::KeyShareFirst,
                clock,
            }
        }
    }

    /// The rustls client of every connection, and the secrets it logs.
    fn client(
        kx_groups: Option<Vec<&'static dyn rustls::crypto::SupportedKxGroup>>,
    ) -> (Arc<ClientConfig>, Arc<Secrets>) {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(pki::root())).unwrap();
        let mut config = ClientConfig::builder_with_provider(provider(kx_groups))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let secrets = Arc::new(Secrets::default());
        config.alpn_protocols = vec![b"h3".to_vec()];
        config.enable_early_data = true;
        config.key_log = Arc::clone(&secrets) as Arc<dyn KeyLog>;
        (Arc::new(config), secrets)
    }

    /// One connection between the server's engine and a rustls client.
    struct Connection {
        events: Vec<Event>,
        end: Option<Result<(), String>>,
        peer: Peer,
        parameters: ServerParameters,
        logged: Logged,
    }

    fn connect(server: &Server, client: &Arc<ClientConfig>) -> Connection {
        let mut targets = SniTargets::new();
        targets.insert("example.com", "web", vec![h3()]);
        let identity = CertifiedKey::new(vec![Bytes::from(pki::leaf())], pki::key(2)).unwrap();
        let key_log = Arc::new(Secrets::default());
        let engine = ServerHandshake::new(
            AwsLcBackend,
            PreferenceTable::new(
                vec![
                    CipherSuite::TLS_AES_128_GCM_SHA256,
                    CipherSuite::TLS_AES_256_GCM_SHA384,
                ],
                server.groups.clone(),
                server.group_preference,
            ),
            server.tickets.clone(),
            PassThroughLayers,
            targets,
            identity,
            NoCertificateRequest,
            Arc::clone(&server.clock),
            Arc::clone(&key_log),
            ServerHandshakeSettings::default(),
        );
        let mut call = pin!(LentCall::<ServerSide, _>::new(ServerSteps::new(
            engine,
            MessageLimits::default()
        )));
        let mut peer = Peer::new(
            quic::ClientConnection::new(
                Arc::clone(client),
                Version::V1,
                ServerName::try_from("example.com").unwrap(),
                CLIENT_PARAMETERS.to_vec(),
            )
            .unwrap(),
        );
        let mut side = ServerSide {
            input: Incoming::default(),
            output: Outgoing::default(),
            parameters: ServerParameters {
                local: Bytes::from_static(SERVER_PARAMETERS),
                peer: None,
                early_data: true,
                refuse: false,
            },
        };
        let (events, end) = run(&mut peer, &mut side, |side| {
            let mut side = pin!(Pin::new(side).view());
            call.as_mut()
                .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
        });
        let Side {
            input, parameters, ..
        } = side;
        // Every byte rustls sent was read at its level.
        assert!(input.levels.iter().all(|level| level.is_empty()));
        Connection {
            events,
            end: end.map(|end| end.map_err(|error| format!("{error:?}"))),
            peer,
            parameters,
            logged: key_log.take(),
        }
    }

    /// Checks the secrets and facts of a completed connection against what
    /// rustls logged; whether the server resumed and accepted 0-RTT.
    fn check(connection: &Connection, logged: &Logged) -> (bool, bool) {
        assert_eq!(connection.end, Some(Ok(())));
        let [
            Event::ServerHelloWritten(ServerHelloWritten {
                aead,
                secrets,
                selected: selected_at_hello,
                zero_rtt,
            }),
            Event::ServerFinishedWritten(one_rtt_send),
            Event::Completed(ServerHandshakeCompleted {
                one_rtt_receive,
                exporter,
                selected,
                resumed,
            }),
        ] = connection.events.as_slice()
        else {
            panic!("unexpected events")
        };
        assert_eq!(*aead, AeadAlgorithm::Aes128Gcm);
        logged.check("SERVER_HANDSHAKE_TRAFFIC_SECRET", &secrets.send);
        logged.check("CLIENT_HANDSHAKE_TRAFFIC_SECRET", &secrets.receive);
        logged.check("SERVER_TRAFFIC_SECRET_0", &one_rtt_send.0);
        logged.check("CLIENT_TRAFFIC_SECRET_0", &one_rtt_receive.0);
        if let Some(zero_rtt) = zero_rtt {
            logged.check("CLIENT_EARLY_TRAFFIC_SECRET", &zero_rtt.0);
        }
        connection.logged.check_engine(logged, zero_rtt.is_some());
        let label = ExporterLabel::new(EXPORTER_LABEL).unwrap();
        connection.peer.check_agreed(
            |context| {
                exporter
                    .export(&label, context, ExpandLength::new(32).unwrap())
                    .as_bytes()
                    .to_vec()
            },
            &selected.application_protocol,
            connection.parameters.peer.as_deref().unwrap(),
        );
        assert_eq!(selected.target, "web");
        assert_eq!(selected_at_hello, selected);
        let client = connection.peer.client();
        assert_eq!(client.tls13_tickets_received(), 2);
        assert_eq!(client.is_early_data_accepted(), zero_rtt.is_some());
        (*resumed, zero_rtt.is_some())
    }

    #[test]
    fn a_full_handshake_with_a_rustls_client() {
        let server = Server::new();
        let (client, secrets) = client(None);
        let connection = connect(&server, &client);
        assert_eq!(check(&connection, &secrets.take()), (false, false));
        assert_eq!(
            connection.peer.connection.handshake_kind(),
            Some(HandshakeKind::Full)
        );
    }

    #[test]
    fn a_rustls_client_retries_with_the_group_the_server_asks_for() {
        let mut server = Server::new();
        server.groups = vec![NamedGroup::SECP256R1, NamedGroup::X25519];
        server.group_preference = GroupPreference::TableOrder;
        let (client, secrets) = client(None);
        let connection = connect(&server, &client);
        assert_eq!(check(&connection, &secrets.take()), (false, false));
        let peer = &connection.peer.connection;
        assert_eq!(
            peer.handshake_kind(),
            Some(HandshakeKind::FullWithHelloRetryRequest)
        );
        assert_eq!(
            peer.negotiated_key_exchange_group().unwrap().name(),
            kx_group::SECP256R1.name()
        );
    }

    #[test]
    fn a_rustls_client_resumes_with_0_rtt() {
        let server = Server::new();
        let (client, secrets) = client(None);
        let first = connect(&server, &client);
        assert_eq!(check(&first, &secrets.take()), (false, false));
        let second = connect(&server, &client);
        assert_eq!(check(&second, &secrets.take()), (true, true));
        assert_eq!(
            second.peer.connection.handshake_kind(),
            Some(HandshakeKind::Resumed)
        );
    }
}

mod client_engine {
    //! The client's engine against a rustls server.

    use super::*;

    type Event = steps::ClientEvent<AwsLcBackend>;

    /// What the client keeps across connections.
    struct Client {
        clock: Arc<ManualClock<MutexSynchronization>>,
        tickets: Arc<
            TakeOnceTicketMap<
                Arc<ManualClock<MutexSynchronization>>,
                MutexSynchronization,
                RandomState,
            >,
        >,
        key_shares: Vec<NamedGroup>,
    }

    impl Client {
        fn new() -> Self {
            let clock = Arc::new(ManualClock::new(now()));
            Self {
                tickets: Arc::new(TakeOnceTicketMap::new(
                    Arc::clone(&clock),
                    Duration::from_secs(86_400),
                    4,
                )),
                key_shares: vec![NamedGroup::X25519],
                clock,
            }
        }
    }

    /// The rustls server of every connection, and the secrets it logs;
    /// with `request_certificate` it sends a CertificateRequest and accepts
    /// a client without a certificate.
    fn server(
        kx_groups: Option<Vec<&'static dyn rustls::crypto::SupportedKxGroup>>,
        request_certificate: bool,
    ) -> (Arc<ServerConfig>, Arc<Secrets>) {
        let provider = provider(kx_groups);
        let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap();
        let builder = if request_certificate {
            let mut roots = RootCertStore::empty();
            roots.add(CertificateDer::from(pki::root())).unwrap();
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .allow_unauthenticated()
            .build()
            .unwrap();
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let mut config = builder
            .with_single_cert(
                vec![CertificateDer::from(pki::leaf())],
                PrivateKeyDer::Pkcs8(pki::pkcs8(2).into()),
            )
            .unwrap();
        let secrets = Arc::new(Secrets::default());
        config.alpn_protocols = vec![b"h3".to_vec()];
        config.max_early_data_size = u32::MAX;
        config.key_log = Arc::clone(&secrets) as Arc<dyn KeyLog>;
        (Arc::new(config), secrets)
    }

    /// One connection between the client's engine and a rustls server.
    struct Connection {
        events: Vec<Event>,
        end: Option<Result<core::convert::Infallible, String>>,
        peer: Peer,
        parameters: ClientParameters,
        logged: Logged,
    }

    fn connect(client: &Client, server: &Arc<ServerConfig>) -> Connection {
        let key_log = Arc::new(Secrets::default());
        let engine = ClientHandshake::new(
            AwsLcBackend,
            PreferenceOffering::<MutexSynchronization, RandomState>::new(
                vec![
                    CipherSuite::TLS_AES_128_GCM_SHA256,
                    CipherSuite::TLS_AES_256_GCM_SHA384,
                ],
                vec![NamedGroup::X25519, NamedGroup::SECP256R1],
                client.key_shares.clone(),
            )
            .unwrap(),
            Arc::clone(&client.tickets),
            verifier(&client.clock),
            NoClientCertificate,
            Arc::clone(&client.clock),
            Arc::clone(&key_log),
            ServerTarget::from_host_name("example.com").unwrap(),
            Some(ProtocolNameList::new(vec![h3()]).unwrap()),
            ClientHandshakeSettings::default(),
        );
        let mut call = pin!(LentCall::<ClientSide, _>::new(ClientSteps::new(engine)));
        let mut peer = Peer::new(
            quic::ServerConnection::new(
                Arc::clone(server),
                Version::V1,
                SERVER_PARAMETERS.to_vec(),
            )
            .unwrap(),
        );
        let mut side = ClientSide {
            input: Incoming::default(),
            output: Outgoing::default(),
            parameters: ClientParameters {
                local: Bytes::from_static(CLIENT_PARAMETERS),
                peer: None,
            },
        };
        let (events, end) = run(&mut peer, &mut side, |side| {
            let mut side = pin!(Pin::new(side).view());
            call.as_mut()
                .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
        });
        let Side {
            input, parameters, ..
        } = side;
        // Every byte rustls sent was read at its level, the tickets too.
        assert!(input.levels.iter().all(|level| level.is_empty()));
        assert_eq!(input.current, CryptoLevel::OneRtt);
        Connection {
            events,
            end: end.map(|end| end.map_err(|error| format!("{error:?}"))),
            peer,
            parameters,
            logged: key_log.take(),
        }
    }

    /// Checks the secrets and facts of a completed connection against what
    /// rustls logged; the server's identity and the outcome of 0-RTT.
    fn check(connection: &Connection, logged: &Logged) -> (ServerIdentity, EarlyDataOutcome) {
        // The client keeps reading tickets.
        assert_eq!(connection.end, None);
        let mut events = connection.events.as_slice();
        let mut early_offered = false;
        if let [
            Event::EarlyDataOffered(EarlyDataOffered { aead, secret, .. }),
            rest @ ..,
        ] = events
        {
            assert_eq!(*aead, AeadAlgorithm::Aes128Gcm);
            logged.check("CLIENT_EARLY_TRAFFIC_SECRET", &secret.0);
            early_offered = true;
            events = rest;
        }
        connection.logged.check_engine(logged, early_offered);
        let [
            Event::ServerHelloRead(ServerHelloRead {
                aead,
                secrets: handshake,
            }),
            Event::Completed(ClientHandshakeCompleted {
                secrets,
                exporter,
                identity,
                application_protocol,
                early_data,
            }),
        ] = events
        else {
            panic!("unexpected events")
        };
        assert_eq!(*aead, AeadAlgorithm::Aes128Gcm);
        logged.check("CLIENT_HANDSHAKE_TRAFFIC_SECRET", &handshake.send);
        logged.check("SERVER_HANDSHAKE_TRAFFIC_SECRET", &handshake.receive);
        logged.check("CLIENT_TRAFFIC_SECRET_0", &secrets.send);
        logged.check("SERVER_TRAFFIC_SECRET_0", &secrets.receive);
        let label = ExporterLabel::new(EXPORTER_LABEL).unwrap();
        connection.peer.check_agreed(
            |context| {
                exporter
                    .export(&label, context, ExpandLength::new(32).unwrap())
                    .as_bytes()
                    .to_vec()
            },
            application_protocol,
            connection.parameters.peer.as_deref().unwrap(),
        );
        (identity.clone(), *early_data)
    }

    #[test]
    fn a_full_handshake_with_a_rustls_server() {
        let client = Client::new();
        let (server, secrets) = server(None, false);
        let connection = connect(&client, &server);
        assert_eq!(
            check(&connection, &secrets.take()),
            (
                ServerIdentity::Verified(leaf_key()),
                EarlyDataOutcome::NotOffered
            )
        );
        assert_eq!(
            connection.peer.connection.handshake_kind(),
            Some(HandshakeKind::Full)
        );
    }

    #[test]
    fn the_client_answers_a_certificate_request_with_an_empty_certificate() {
        let client = Client::new();
        let (server, secrets) = server(None, true);
        let connection = connect(&client, &server);
        assert_eq!(
            check(&connection, &secrets.take()),
            (
                ServerIdentity::Verified(leaf_key()),
                EarlyDataOutcome::NotOffered
            )
        );
        let quic::Connection::Server(peer) = &connection.peer.connection else {
            panic!("the peer is not a server")
        };
        assert!(peer.peer_certificates().is_none());
    }

    #[test]
    fn the_client_retries_with_the_group_a_rustls_server_asks_for() {
        let client = Client::new();
        let (server, secrets) = server(Some(vec![kx_group::SECP256R1]), false);
        let connection = connect(&client, &server);
        assert_eq!(
            check(&connection, &secrets.take()),
            (
                ServerIdentity::Verified(leaf_key()),
                EarlyDataOutcome::NotOffered
            )
        );
        let peer = &connection.peer.connection;
        assert_eq!(
            peer.handshake_kind(),
            Some(HandshakeKind::FullWithHelloRetryRequest)
        );
        assert_eq!(
            peer.negotiated_key_exchange_group().unwrap().name(),
            kx_group::SECP256R1.name()
        );
    }

    #[test]
    fn the_client_resumes_a_rustls_ticket_with_0_rtt() {
        let client = Client::new();
        let (server, secrets) = server(None, false);
        let first = connect(&client, &server);
        check(&first, &secrets.take());
        let second = connect(&client, &server);
        assert_eq!(
            check(&second, &secrets.take()),
            (
                ServerIdentity::Resumed(leaf_key()),
                EarlyDataOutcome::Accepted
            )
        );
        assert_eq!(
            second.peer.connection.handshake_kind(),
            Some(HandshakeKind::Resumed)
        );
    }
}
