//! The server's and the client's handshake engines run against each other,
//! over CRYPTO streams in memory.
//!
//! Each side has a byte buffer per level for what it receives and for what
//! it sends; between polls, what one side sent moves to the other side's
//! buffers of the same level, where a test may change it on the way. The
//! server presents an Ed25519 certificate for example.com, issued by an
//! Ed25519 root that the client's verifier alone trusts; both are built
//! here and signed with nitori_crypto_aws_lc.
#![feature(impl_trait_in_assoc_type, yield_expr)]

mod common;

use bytes::Bytes;
use common::{
    now,
    pki::{self, leaf_key, verifier},
    ready,
    steps::{self, ClientSteps, ServerSteps, ServerStepsFrom},
    streams::{
        ClientParameters, ClientSide, Fault, Incoming, LEVELS, Outgoing, ServerParameters,
        ServerSide, index,
    },
};
use core::{
    ops::ControlFlow,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use nitori_call::{LentCall, Receiver};
use nitori_crypto::{ExpandLength, NamedGroup};
use nitori_crypto::{Hash as _, HashAlgorithm, KeyExchange, SignatureScheme, StartedExchange as _};
use nitori_crypto_aws_lc::AwsLcBackend;
use nitori_sync::MutexSynchronization;
use nitori_tls::{
    alert::AlertDescription,
    cipher_suite::CipherSuite,
    client::{
        ServerTarget,
        engine::{
            ClientAlertError, ClientHandshake, ClientHandshakeCompleted, ClientHandshakeSettings,
            EarlyDataOffered, EarlyDataOutcome, ServerHelloRead, ServerIdentity,
            WriteClientHelloStep,
        },
        identity::NoClientCertificate,
        offering::{Offering, PreferenceOffering},
        ticket::TakeOnceTicketMap,
    },
    clock::ManualClock,
    engine::{CryptoLevel, HandshakeError, ProtocolViolationError},
    extension::alpn::{ProtocolName, ProtocolNameList},
    message::{HandshakeType, new_session_ticket::TicketLifetime},
    schedule::{CertificateVerifyError, ExporterLabel, FinishedError},
    server::{
        choice::{GroupPreference, PreferenceTable},
        engine::{
            ReadFirstClientHello, RequiredExtension, ServerAlertError, ServerHandshake,
            ServerHandshakeCompleted, ServerHandshakeSettings, ServerHelloWritten,
            WriteServerHelloStep,
        },
        identity::CertifiedKey,
        layers::{
            AcceptedHello as _, HelloLayers, PassThroughHello, PassThroughLayers, PreopenedLayers,
        },
        peer::NoCertificateRequest,
        targets::{PreselectedTarget, SniTargets, TargetSelection as _},
        ticket::ZeroRttOnceTicketTable,
    },
};
use nitori_tls::{
    code_point::{ByteCodePointList, CodePointList},
    extension::{
        OpaqueData,
        key_share::{ClientShares, KeyShareEntry},
        psk_key_exchange_modes::PskKeyExchangeMode,
        server_name::ServerName,
    },
    message::{
        HandshakeMessage, HelloRandom, LegacySessionId, MessageLimits,
        client_hello::{ClientHello, ClientHelloExtension, codec::write_client_hello},
        codec::ReadHandshakeMessageHostExt as _,
        server_hello::{ServerHelloMessage, codec::read_server_hello_message},
    },
    role::Client,
    schedule::{Authentication, HandshakeSecret, Transcript},
    version::ProtocolVersion,
};
use std::{
    collections::VecDeque,
    hash::RandomState,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Who sent bytes on their way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sender {
    Server,
    Client,
}

/// The receivers of both sides.
struct Wire {
    server: ServerSide,
    client: ClientSide,
    /// The messages that went past, by sender and level.
    seen: Vec<(Sender, CryptoLevel, HandshakeType)>,
}

impl Wire {
    fn new() -> Self {
        Self {
            server: ServerSide {
                input: Incoming::default(),
                output: Outgoing::default(),
                parameters: ServerParameters {
                    local: Bytes::from_static(b"server parameters"),
                    peer: None,
                    early_data: true,
                    refuse: false,
                },
            },
            client: ClientSide {
                input: Incoming::default(),
                output: Outgoing::default(),
                parameters: ClientParameters {
                    local: Bytes::from_static(b"client parameters"),
                    peer: None,
                },
            },
            seen: Vec::new(),
        }
    }

    /// Moves what each side sent to the other, through `change`; whether
    /// anything moved.
    fn deliver(&mut self, change: &mut impl FnMut(Sender, CryptoLevel, &mut Vec<u8>)) -> bool {
        let mut moved = false;
        for (from, out, into) in [
            (
                Sender::Server,
                &mut self.server.output,
                &mut self.client.input,
            ),
            (
                Sender::Client,
                &mut self.client.output,
                &mut self.server.input,
            ),
        ] {
            for level in LEVELS {
                let mut bytes = core::mem::take(&mut out.levels[index(level)]);
                if bytes.is_empty() {
                    continue;
                }
                for (message_type, _) in messages(&bytes) {
                    self.seen.push((from, level, message_type));
                }
                change(from, level, &mut bytes);
                into.levels[index(level)].extend(bytes);
                moved = true;
            }
        }
        moved
    }

    fn seen(&self, from: Sender, message_type: HandshakeType) -> usize {
        self.seen
            .iter()
            .filter(|&&(sender, _, seen)| sender == from && seen == message_type)
            .count()
    }
}

/// The whole messages in `bytes`, with their ranges.
fn messages(bytes: &[u8]) -> Vec<(HandshakeType, core::ops::Range<usize>)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let len = u32::from_be_bytes([0, bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let end = (at + 4 + len).min(bytes.len());
        found.push((HandshakeType::new(bytes[at]), at..end));
        at = end;
    }
    found
}

/// Flips the last byte of each message of `message_type` in `bytes`.
fn flip_last(bytes: &mut [u8], message_type: HandshakeType) {
    for (found, range) in messages(bytes) {
        if found == message_type {
            bytes[range.end - 1] ^= 1;
        }
    }
}

/// What a run of both sides left.
struct Run<SE, SF, CE, CF> {
    server_events: Vec<SE>,
    server_end: Option<SF>,
    client_events: Vec<CE>,
    client_end: Option<CF>,
}

/// Polls `server` and `client` in turns, delivering what they send through
/// `change`, until neither moves.
fn run<SE, SF, CE, CF>(
    wire: &mut Wire,
    mut server: impl FnMut(&mut Wire) -> Poll<ControlFlow<SF, SE>>,
    mut client: impl FnMut(&mut Wire) -> Poll<ControlFlow<CF, CE>>,
    mut change: impl FnMut(Sender, CryptoLevel, &mut Vec<u8>),
) -> Run<SE, SF, CE, CF> {
    let mut result = Run {
        server_events: Vec::new(),
        server_end: None,
        client_events: Vec::new(),
        client_end: None,
    };
    for _ in 0..64 {
        let mut progress = false;
        while result.client_end.is_none() {
            match client(wire) {
                Poll::Ready(ControlFlow::Continue(event)) => result.client_events.push(event),
                Poll::Ready(ControlFlow::Break(end)) => result.client_end = Some(end),
                Poll::Pending => break,
            }
            progress = true;
        }
        progress |= wire.deliver(&mut change);
        while result.server_end.is_none() {
            match server(wire) {
                Poll::Ready(ControlFlow::Continue(event)) => result.server_events.push(event),
                Poll::Ready(ControlFlow::Break(end)) => result.server_end = Some(end),
                Poll::Pending => break,
            }
            progress = true;
        }
        progress |= wire.deliver(&mut change);
        if !progress {
            return result;
        }
    }
    panic!("the handshake does not settle");
}

/// What both sides keep across connections.
struct Endpoints {
    clock: Arc<ManualClock<MutexSynchronization>>,
    server_tickets: ZeroRttOnceTicketTable<
        &'static str,
        AwsLcBackend,
        Arc<ManualClock<MutexSynchronization>>,
        MutexSynchronization,
        RandomState,
    >,
    client_tickets: Arc<
        TakeOnceTicketMap<
            Arc<ManualClock<MutexSynchronization>>,
            MutexSynchronization,
            RandomState,
        >,
    >,
    offering: PreferenceOffering<MutexSynchronization, RandomState>,
    server_protocols: Vec<ProtocolName<Bytes>>,
    group_preference: GroupPreference,
    server_groups: Vec<NamedGroup>,
    /// Whether the server serves its one target to every name.
    fallback: bool,
}

impl Endpoints {
    fn new() -> Self {
        let clock = Arc::new(ManualClock::new(now()));
        Self {
            server_tickets: ZeroRttOnceTicketTable::new(
                AwsLcBackend,
                Arc::clone(&clock),
                2,
                TicketLifetime::new(3600).unwrap(),
                true,
                16,
            ),
            client_tickets: Arc::new(TakeOnceTicketMap::new(
                Arc::clone(&clock),
                Duration::from_secs(86_400),
                4,
            )),
            offering: offering(&[NamedGroup::X25519]),
            server_protocols: vec![h3()],
            group_preference: GroupPreference::KeyShareFirst,
            server_groups: vec![NamedGroup::X25519, NamedGroup::SECP256R1],
            fallback: false,
            clock,
        }
    }
}

fn h3() -> ProtocolName<Bytes> {
    ProtocolName::new(Bytes::from_static(b"h3")).unwrap()
}

fn offering(key_shares: &[NamedGroup]) -> PreferenceOffering<MutexSynchronization, RandomState> {
    PreferenceOffering::new(
        vec![
            CipherSuite::TLS_AES_128_GCM_SHA256,
            CipherSuite::TLS_AES_256_GCM_SHA384,
        ],
        vec![NamedGroup::X25519, NamedGroup::SECP256R1],
        key_shares.to_vec(),
    )
    .unwrap()
}

type ServerEvent = steps::ServerEvent<AwsLcBackend, &'static str>;
type ClientEvent = steps::ClientEvent<AwsLcBackend>;

/// The events and final values of one connection between the endpoints.
struct Connection<SF, CF> {
    wire: Wire,
    server_events: Vec<ServerEvent>,
    server_end: Option<SF>,
    client_events: Vec<ClientEvent>,
    client_end: Option<CF>,
}

/// The targets of `endpoints`: example.com, and every name when
/// `fallback` is set.
fn sni_targets(endpoints: &Endpoints) -> SniTargets<&'static str> {
    let mut targets = SniTargets::new();
    targets.insert("example.com", "web", endpoints.server_protocols.clone());
    if endpoints.fallback {
        targets.set_fallback("web", endpoints.server_protocols.clone());
    }
    targets
}

/// The server's handshake of `endpoints`, which serves example.com, with
/// `layers` and `targets` when they are given.
macro_rules! server_handshake {
    ($endpoints:expr) => {{
        let endpoints: &Endpoints = $endpoints;
        server_handshake!(endpoints, PassThroughLayers, sni_targets(endpoints))
    }};
    ($endpoints:expr, $layers:expr, $targets:expr) => {{
        let endpoints: &Endpoints = $endpoints;
        let identity = CertifiedKey::new(vec![Bytes::from(pki::leaf())], pki::key(2)).unwrap();
        ServerHandshake::new(
            AwsLcBackend,
            PreferenceTable::new(
                vec![
                    CipherSuite::TLS_AES_128_GCM_SHA256,
                    CipherSuite::TLS_AES_256_GCM_SHA384,
                ],
                endpoints.server_groups.clone(),
                endpoints.group_preference,
            ),
            endpoints.server_tickets.clone(),
            $layers,
            $targets,
            identity,
            NoCertificateRequest,
            Arc::clone(&endpoints.clock),
            (),
            ServerHandshakeSettings::default(),
        )
    }};
}

/// The client's handshake of `endpoints` to `host`.
macro_rules! client_handshake {
    ($endpoints:expr, $host:expr) => {{
        let endpoints: &Endpoints = $endpoints;
        ClientHandshake::new(
            AwsLcBackend,
            endpoints.offering.clone(),
            Arc::clone(&endpoints.client_tickets),
            verifier(&endpoints.clock),
            NoClientCertificate,
            Arc::clone(&endpoints.clock),
            (),
            ServerTarget::from_host_name($host).unwrap(),
            Some(ProtocolNameList::new(vec![h3()]).unwrap()),
            ClientHandshakeSettings::default(),
        )
    }};
}

/// Polls `server`, a pinned call of [`server_handshake!`], on the server's
/// receivers of `wire`.
macro_rules! poll_server {
    ($server:expr, $wire:expr) => {{
        let wire: &mut Wire = $wire;
        let mut side = pin!(Pin::new(&mut wire.server).view());
        $server
            .as_mut()
            .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
    }};
}

/// Runs one connection to `host` with the server's parameters as `wire`
/// has them, through `change`.
macro_rules! connect {
    ($endpoints:expr, $wire:expr, $host:expr, $change:expr) => {{
        let endpoints: &Endpoints = $endpoints;
        let mut wire: Wire = $wire;
        let server = server_handshake!(endpoints);
        let client = client_handshake!(endpoints, $host);
        let mut server = pin!(LentCall::<ServerSide, _>::new(ServerSteps::new(
            server,
            MessageLimits::default()
        )));
        let mut client = pin!(LentCall::<ClientSide, _>::new(ClientSteps::new(client)));
        let run = run(
            &mut wire,
            |wire| poll_server!(server, wire),
            |wire| {
                let mut side = pin!(Pin::new(&mut wire.client).view());
                client
                    .as_mut()
                    .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
            },
            $change,
        );
        Connection {
            wire,
            server_events: run.server_events,
            server_end: run.server_end,
            client_events: run.client_events,
            client_end: run.client_end,
        }
    }};
}

fn unchanged(_: Sender, _: CryptoLevel, _: &mut Vec<u8>) {}

/// The secrets and facts of a completed connection, checked to agree on
/// both sides.
struct Agreed {
    resumed: bool,
    zero_rtt: bool,
    identity: ServerIdentity,
    early_data: EarlyDataOutcome,
}

fn agreed(server_events: &[ServerEvent], client_events: &[ClientEvent]) -> Agreed {
    let mut server = server_events.iter();
    let mut client = client_events.iter();
    let label = ExporterLabel::new(b"EXPORTER-test").unwrap();
    let len = ExpandLength::new(32).unwrap();

    let Some(ServerEvent::ServerHelloWritten(ServerHelloWritten {
        aead: server_aead,
        secrets: server_secrets,
        selected: selected_at_hello,
        zero_rtt,
    })) = server.next()
    else {
        panic!("no Handshake secrets from the server")
    };
    let mut client_event = client.next();
    let zero_rtt_sent = match client_event {
        Some(ClientEvent::EarlyDataOffered(EarlyDataOffered { aead, secret, .. })) => {
            if let Some(received) = zero_rtt {
                assert_eq!(received.0.as_bytes(), secret.0.as_bytes());
                assert_eq!(aead, server_aead);
            }
            client_event = client.next();
            true
        }
        _ => false,
    };
    let Some(ClientEvent::ServerHelloRead(ServerHelloRead {
        aead: client_aead,
        secrets: client_secrets,
    })) = client_event
    else {
        panic!("no Handshake secrets for the client")
    };
    assert_eq!(server_aead, client_aead);
    assert_eq!(
        server_secrets.send.as_bytes(),
        client_secrets.receive.as_bytes()
    );
    assert_eq!(
        server_secrets.receive.as_bytes(),
        client_secrets.send.as_bytes()
    );
    assert_ne!(
        server_secrets.send.as_bytes(),
        server_secrets.receive.as_bytes()
    );

    let Some(ServerEvent::ServerFinishedWritten(server_send)) = server.next() else {
        panic!("no 1-RTT secret from the server")
    };
    let Some(ServerEvent::Completed(ServerHandshakeCompleted {
        one_rtt_receive,
        exporter: server_exporter,
        selected,
        resumed,
    })) = server.next()
    else {
        panic!("the server did not complete")
    };
    let Some(ClientEvent::Completed(ClientHandshakeCompleted {
        secrets,
        exporter: client_exporter,
        identity,
        application_protocol,
        early_data,
    })) = client.next()
    else {
        panic!("the client did not complete")
    };
    assert!(server.next().is_none() && client.next().is_none());
    assert_eq!(server_send.0.as_bytes(), secrets.receive.as_bytes());
    assert_eq!(one_rtt_receive.0.as_bytes(), secrets.send.as_bytes());
    assert_ne!(secrets.send.as_bytes(), client_secrets.send.as_bytes());
    assert_eq!(
        server_exporter.export(&label, b"context", len).as_bytes(),
        client_exporter.export(&label, b"context", len).as_bytes()
    );
    assert_ne!(
        server_exporter.export(&label, b"context", len).as_bytes(),
        client_exporter.export(&label, b"other", len).as_bytes()
    );
    assert_eq!(selected.target, "web");
    assert_eq!(selected.application_protocol, Some(h3()));
    assert_eq!(selected_at_hello, selected);
    assert_eq!(application_protocol, &Some(h3()));
    assert_eq!(
        zero_rtt.is_some(),
        *early_data == EarlyDataOutcome::Accepted
    );
    if !zero_rtt_sent {
        assert_eq!(*early_data, EarlyDataOutcome::NotOffered);
    }
    Agreed {
        resumed: *resumed,
        zero_rtt: zero_rtt.is_some(),
        identity: identity.clone(),
        early_data: *early_data,
    }
}

/// The alert a side ended with.
fn alert_of<T, F: nitori_tls::alert::CallsForAlert, E: core::fmt::Debug>(
    end: Option<Result<T, HandshakeError<F, E>>>,
) -> AlertDescription {
    match end {
        Some(Err(error)) => error.alert().expect("an alert"),
        Some(Ok(_)) => panic!("the side succeeded"),
        None => panic!("the side did not end"),
    }
}

/// With the family of mutexes, the ready-made capabilities that connections
/// share can be shared between threads.
#[test]
fn the_shared_capabilities_of_the_mutex_family_are_send_and_sync() {
    use nitori_tls::verify::{AnchorStore, SpkiDistrustList, transparency::EmbeddedSctPolicy};
    type Clock = Arc<ManualClock<MutexSynchronization>>;
    fn shared<T: Send + Sync>() {}
    shared::<ManualClock<MutexSynchronization>>();
    shared::<TakeOnceTicketMap<Clock, MutexSynchronization, RandomState>>();
    shared::<PreferenceOffering<MutexSynchronization, RandomState>>();
    shared::<
        ZeroRttOnceTicketTable<
            &'static str,
            AwsLcBackend,
            Clock,
            MutexSynchronization,
            RandomState,
        >,
    >();
    shared::<PreopenedLayers<PassThroughLayers, MutexSynchronization>>();
    shared::<AnchorStore<MutexSynchronization>>();
    shared::<SpkiDistrustList<MutexSynchronization>>();
    shared::<EmbeddedSctPolicy<MutexSynchronization>>();
}

#[test]
fn the_calls_of_both_engines_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let endpoints = Endpoints::new();
    let server = LentCall::<ServerSide, _>::new(ServerSteps::new(
        server_handshake!(&endpoints),
        MessageLimits::default(),
    ));
    assert_send(&server);
    let client = LentCall::<ClientSide, _>::new(ClientSteps::new(client_handshake!(
        &endpoints,
        "example.com"
    )));
    assert_send(&client);
}

#[test]
fn a_step_runs_alone_as_an_operation() {
    let endpoints = Endpoints::new();
    let mut wire = Wire::new();
    let mut client = pin!(LentCall::<ClientSide, _>::new(WriteClientHelloStep::new(
        client_handshake!(&endpoints, "example.com")
    )));
    let mut side = pin!(Pin::new(&mut wire.client).view());
    let Poll::Ready(ControlFlow::Break(Ok((offered, _)))) = client
        .as_mut()
        .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
    else {
        panic!("the ClientHello is not written")
    };
    assert!(offered.is_none());
    assert!(wire.deliver(&mut unchanged));

    // The first ClientHello is read without the engine.
    let mut read = pin!(LentCall::<ServerSide, _>::new(ReadFirstClientHello::new(
        MessageLimits::default()
    )));
    let mut side = pin!(Pin::new(&mut wire.server).view());
    let Poll::Ready(ControlFlow::Break(Ok(first))) = read
        .as_mut()
        .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
    else {
        panic!("the ClientHello is not read")
    };
    assert!(first.client_hello().pre_shared_key().is_none());
    assert_eq!(wire.server.output.current, CryptoLevel::Initial);

    let mut server = pin!(LentCall::<ServerSide, _>::new(WriteServerHelloStep::new(
        server_handshake!(&endpoints),
        first
    )));
    let mut side = pin!(Pin::new(&mut wire.server).view());
    let Poll::Ready(ControlFlow::Break(Ok((written, _)))) = server
        .as_mut()
        .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
    else {
        panic!("the ServerHello is not written")
    };
    assert!(written.zero_rtt.is_none());
    // The step ends after the ServerHello, before moving to Handshake.
    assert_eq!(wire.server.output.current, CryptoLevel::Initial);
    assert!(wire.deliver(&mut unchanged));
    assert_eq!(
        wire.seen,
        [
            (
                Sender::Client,
                CryptoLevel::Initial,
                HandshakeType::CLIENT_HELLO
            ),
            (
                Sender::Server,
                CryptoLevel::Initial,
                HandshakeType::SERVER_HELLO
            ),
        ]
    );
}

#[test]
fn a_full_handshake_agrees_on_every_secret() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert!(matches!(connection.server_end, Some(Ok(()))));
    // The client keeps reading tickets.
    assert!(connection.client_end.is_none());
    let agreed = agreed(&connection.server_events, &connection.client_events);
    assert!(!agreed.resumed && !agreed.zero_rtt);
    assert_eq!(agreed.identity, ServerIdentity::Verified(leaf_key()));
    assert_eq!(agreed.early_data, EarlyDataOutcome::NotOffered);
    let wire = &connection.wire;
    assert_eq!(
        wire.server.parameters.peer.as_deref(),
        Some(&b"client parameters"[..])
    );
    assert_eq!(
        wire.client.parameters.peer.as_deref(),
        Some(&b"server parameters"[..])
    );
    assert_eq!(wire.seen(Sender::Server, HandshakeType::SERVER_HELLO), 1);
    assert_eq!(wire.seen(Sender::Server, HandshakeType::CERTIFICATE), 1);
    assert_eq!(
        wire.seen(Sender::Server, HandshakeType::CERTIFICATE_VERIFY),
        1
    );
    assert_eq!(
        wire.seen(Sender::Server, HandshakeType::NEW_SESSION_TICKET),
        2
    );
    assert!(wire.seen.contains(&(
        Sender::Client,
        CryptoLevel::Handshake,
        HandshakeType::FINISHED
    )));
    assert!(wire.seen.contains(&(
        Sender::Server,
        CryptoLevel::OneRtt,
        HandshakeType::NEW_SESSION_TICKET
    )));
    // Every byte was read at its level.
    for side in [&wire.server.input, &wire.client.input] {
        assert!(side.levels.iter().all(VecDeque::is_empty));
        assert_eq!(side.current, CryptoLevel::OneRtt);
    }
}

#[test]
fn a_hello_retry_request_moves_the_key_share() {
    let mut endpoints = Endpoints::new();
    endpoints.offering = offering(&[NamedGroup::SECP256R1]);
    endpoints.group_preference = GroupPreference::TableOrder;
    let connection = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert!(matches!(connection.server_end, Some(Ok(()))));
    let agreed = agreed(&connection.server_events, &connection.client_events);
    assert!(!agreed.resumed);
    let wire = &connection.wire;
    // The HelloRetryRequest and the ServerHello, after two ClientHellos.
    assert_eq!(wire.seen(Sender::Server, HandshakeType::SERVER_HELLO), 2);
    assert_eq!(wire.seen(Sender::Client, HandshakeType::CLIENT_HELLO), 2);
    // The offering remembers the group the server asked for.
    let target = ServerTarget::from_host_name("example.com").unwrap();
    let support = verifier(&endpoints.clock).support();
    let (offer, _) =
        Offering::<AwsLcBackend>::offer(&endpoints.offering, &AwsLcBackend, &target, &support)
            .unwrap();
    let shared: Vec<_> = offer
        .key_share_groups()
        .iter()
        .map(|group| group.algorithm())
        .collect();
    assert_eq!(shared, [NamedGroup::X25519]);
    let again = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert_eq!(
        again.wire.seen(Sender::Server, HandshakeType::SERVER_HELLO),
        1
    );
}

use nitori_tls::verify::CertificateVerifier as _;

/// Layers that pass the hello through and record the encoding of each
/// `first` they are given.
struct RecordingLayers(Arc<Mutex<Vec<Option<Vec<u8>>>>>);

impl HelloLayers for RecordingLayers {
    type Accepted = PassThroughHello;
    type Error = core::convert::Infallible;
    type Opening<'a> = core::future::Ready<Result<PassThroughHello, core::convert::Infallible>>;

    fn open<'a>(
        &'a self,
        encoded: HandshakeMessage,
        hello: ClientHello<Bytes>,
        first: Option<&'a PassThroughHello>,
    ) -> Self::Opening<'a> {
        self.0
            .lock()
            .unwrap()
            .push(first.map(|first| first.encoded().encoded().to_vec()));
        PassThroughLayers.open(encoded, hello, None)
    }
}

#[test]
fn a_deciding_server_hands_its_openings_to_the_engine_across_a_retry() {
    let mut endpoints = Endpoints::new();
    endpoints.offering = offering(&[NamedGroup::SECP256R1]);
    endpoints.group_preference = GroupPreference::TableOrder;
    let mut wire = Wire::new();
    let mut client = pin!(LentCall::<ClientSide, _>::new(ClientSteps::new(
        client_handshake!(&endpoints, "example.com")
    )));
    let mut poll_client = |wire: &mut Wire| {
        let mut side = pin!(Pin::new(&mut wire.client).view());
        client
            .as_mut()
            .poll_with_views(side.as_mut(), &mut Context::from_waker(Waker::noop()))
    };
    assert!(poll_client(&mut wire).is_pending());
    assert!(wire.deliver(&mut unchanged));

    // The server reads the first ClientHello, opens it and selects its
    // target before it builds the engine.
    let mut read = pin!(LentCall::<ServerSide, _>::new(ReadFirstClientHello::new(
        MessageLimits::default()
    )));
    let Poll::Ready(ControlFlow::Break(Ok(first))) = poll_server!(read, &mut wire) else {
        panic!("the ClientHello is not read")
    };
    let (encoded, hello) = first.clone().into_parts();
    let Ok(opened) = ready(PassThroughLayers.open(encoded, hello, None));
    let selected = ready(sni_targets(&endpoints).select(opened.client_hello())).unwrap();

    let firsts = Arc::new(Mutex::new(Vec::new()));
    let server = server_handshake!(
        &endpoints,
        PreopenedLayers::<_, MutexSynchronization>::new(
            opened,
            RecordingLayers(Arc::clone(&firsts))
        ),
        PreselectedTarget::new(selected)
    );
    fn assert_send<T: Send>(_: &T) {}
    let server = LentCall::<ServerSide, _>::new(ServerStepsFrom::new(server, first.clone()));
    // The layers are shared across the engine's waits, and stay `Send`.
    assert_send(&server);
    let mut server = pin!(server);
    let run = run(
        &mut wire,
        |wire| poll_server!(server, wire),
        poll_client,
        unchanged,
    );
    assert!(matches!(run.server_end, Some(Ok(()))));
    assert_eq!(wire.seen(Sender::Server, HandshakeType::SERVER_HELLO), 2);
    // The wrapped layers opened the second ClientHello alone, with the
    // opening of the first.
    assert_eq!(
        *firsts.lock().unwrap(),
        [Some(first.encoded().encoded().to_vec())]
    );
}

#[test]
fn a_ticket_resumes_with_0_rtt_accepted() {
    let endpoints = Endpoints::new();
    let first = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    agreed(&first.server_events, &first.client_events);
    let second = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert!(matches!(second.server_end, Some(Ok(()))));
    assert!(matches!(
        second.client_events.first(),
        Some(ClientEvent::EarlyDataOffered(EarlyDataOffered { transport_parameters, application_protocol, .. }))
            if transport_parameters.as_ref() == b"server parameters"
                && application_protocol == &Some(h3())
    ));
    let resumed = agreed(&second.server_events, &second.client_events);
    assert!(resumed.resumed && resumed.zero_rtt);
    assert_eq!(resumed.early_data, EarlyDataOutcome::Accepted);
    assert_eq!(resumed.identity, ServerIdentity::Resumed(leaf_key()));
    let wire = &second.wire;
    assert_eq!(wire.seen(Sender::Server, HandshakeType::CERTIFICATE), 0);
    assert_eq!(
        wire.seen(Sender::Server, HandshakeType::CERTIFICATE_VERIFY),
        0
    );
    // The resumed connection issues tickets of its own, which resume again.
    assert_eq!(
        wire.seen(Sender::Server, HandshakeType::NEW_SESSION_TICKET),
        2
    );
    let third = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let agreed = agreed(&third.server_events, &third.client_events);
    assert!(agreed.resumed);
    assert_eq!(agreed.identity, ServerIdentity::Resumed(leaf_key()));
}

#[test]
fn the_connection_can_reject_0_rtt() {
    let endpoints = Endpoints::new();
    connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let mut wire = Wire::new();
    wire.server.parameters.early_data = false;
    let second = connect!(&endpoints, wire, "example.com", unchanged);
    assert!(matches!(second.server_end, Some(Ok(()))));
    let agreed = agreed(&second.server_events, &second.client_events);
    assert!(agreed.resumed && !agreed.zero_rtt);
    assert_eq!(agreed.early_data, EarlyDataOutcome::Rejected);
}

#[test]
fn a_retry_rejects_0_rtt_but_resumes() {
    let mut endpoints = Endpoints::new();
    connect!(&endpoints, Wire::new(), "example.com", unchanged);
    // The server now wants a group the client does not share.
    endpoints.server_groups = vec![NamedGroup::SECP256R1, NamedGroup::X25519];
    endpoints.group_preference = GroupPreference::TableOrder;
    let second = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert!(matches!(second.server_end, Some(Ok(()))));
    assert_eq!(
        second
            .wire
            .seen(Sender::Server, HandshakeType::SERVER_HELLO),
        2
    );
    let agreed = agreed(&second.server_events, &second.client_events);
    assert!(agreed.resumed && !agreed.zero_rtt);
    assert_eq!(agreed.early_data, EarlyDataOutcome::Rejected);
}

#[test]
fn a_ticket_without_0_rtt_resumes_without_offering_it() {
    let mut endpoints = Endpoints::new();
    endpoints.server_tickets = ZeroRttOnceTicketTable::new(
        AwsLcBackend,
        Arc::clone(&endpoints.clock),
        1,
        TicketLifetime::new(3600).unwrap(),
        false,
        16,
    );
    connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let second = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let agreed = agreed(&second.server_events, &second.client_events);
    assert!(agreed.resumed && !agreed.zero_rtt);
    assert_eq!(agreed.early_data, EarlyDataOutcome::NotOffered);
}

#[test]
fn a_changed_server_finished_fails_with_decrypt_error() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       level,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Server && level == CryptoLevel::Handshake {
            flip_last(bytes, HandshakeType::FINISHED);
        }
    });
    let end = connection.client_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ClientAlertError::Finished {
            source: FinishedError::Mismatch
        })
    ));
    assert_eq!(alert_of(end), AlertDescription::DECRYPT_ERROR);
    assert!(connection.server_end.is_none());
}

#[test]
fn a_changed_client_finished_fails_with_decrypt_error() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       level,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client && level == CryptoLevel::Handshake {
            flip_last(bytes, HandshakeType::FINISHED);
        }
    });
    let end = connection.server_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ServerAlertError::Finished {
            source: FinishedError::Mismatch
        })
    ));
    assert_eq!(alert_of(end), AlertDescription::DECRYPT_ERROR);
    // The server completes nothing.
    assert!(!connection.server_events.iter().any(|event| matches!(
        event,
        ServerEvent::Completed(ServerHandshakeCompleted { .. })
    )));
}

#[test]
fn a_changed_signature_fails_with_decrypt_error() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       _,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Server {
            flip_last(bytes, HandshakeType::CERTIFICATE_VERIFY);
        }
    });
    let end = connection.client_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ClientAlertError::CertificateVerify {
            source: CertificateVerifyError::Signature { .. }
        })
    ));
    assert_eq!(alert_of(end), AlertDescription::DECRYPT_ERROR);
}

#[test]
fn a_scheme_that_does_not_fit_the_key_fails_with_illegal_parameter() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       _,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Server {
            for (found, range) in messages(bytes) {
                if found == HandshakeType::CERTIFICATE_VERIFY {
                    // ecdsa_secp256r1_sha256, which the client offers, for
                    // the Ed25519 key of the certificate.
                    bytes[range.start + 4..range.start + 6].copy_from_slice(&[0x04, 0x03]);
                }
            }
        }
    });
    let end = connection.client_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ClientAlertError::SchemeKey { .. })
    ));
    assert_eq!(alert_of(end), AlertDescription::ILLEGAL_PARAMETER);
}

#[test]
fn without_a_common_application_protocol_the_server_fails() {
    let mut endpoints = Endpoints::new();
    endpoints.server_protocols = vec![ProtocolName::new(Bytes::from_static(b"h2")).unwrap()];
    let connection = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    assert_eq!(
        alert_of(connection.server_end),
        AlertDescription::NO_APPLICATION_PROTOCOL
    );
    assert!(connection.server_events.is_empty());
}

#[test]
fn a_certificate_for_another_name_fails_with_bad_certificate() {
    let mut endpoints = Endpoints::new();
    // The server serves its certificate for example.com to every name.
    endpoints.fallback = true;
    let connection = connect!(&endpoints, Wire::new(), "example.net", unchanged);
    assert_eq!(
        alert_of(connection.client_end),
        AlertDescription::BAD_CERTIFICATE
    );
}

#[test]
fn a_changed_binder_fails_with_decrypt_error() {
    let endpoints = Endpoints::new();
    connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let second = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                   _,
                                                                   bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client {
            // The binder ends the ClientHello.
            flip_last(bytes, HandshakeType::CLIENT_HELLO);
        }
    });
    let end = second.server_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ServerAlertError::Binder { .. })
    ));
    assert_eq!(alert_of(end), AlertDescription::DECRYPT_ERROR);
}

#[test]
fn refused_transport_parameters_fail_through_the_connection() {
    let endpoints = Endpoints::new();
    let mut wire = Wire::new();
    wire.server.parameters.refuse = true;
    let connection = connect!(&endpoints, wire, "example.com", unchanged);
    match connection.server_end {
        Some(Err(error)) => {
            assert_eq!(error.transport(), Some(&Fault::Parameters));
            assert!(error.alert().is_none());
        }
        _ => panic!("the server did not fail"),
    }
}

#[test]
fn bytes_left_unread_at_a_level_fail_through_the_connection() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       level,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client && level == CryptoLevel::Initial {
            // The start of a message after the ClientHello.
            bytes.push(HandshakeType::CLIENT_HELLO.get());
        }
    });
    match connection.server_end {
        Some(Err(error)) => {
            assert_eq!(
                error.transport(),
                Some(&Fault::Unread(CryptoLevel::Initial))
            );
        }
        _ => panic!("the server did not fail"),
    }
}

#[test]
fn a_key_update_after_the_handshake_fails_with_unexpected_message() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       level,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Server && level == CryptoLevel::OneRtt {
            // KeyUpdate, which QUIC does not carry (RFC 9001 §6).
            bytes.extend([HandshakeType::KEY_UPDATE.get(), 0, 0, 1, 0]);
        }
    });
    assert!(matches!(connection.server_end, Some(Ok(()))));
    assert_eq!(
        alert_of(connection.client_end),
        AlertDescription::UNEXPECTED_MESSAGE
    );
}

/// Replaces the first occurrence of `pattern` in `bytes` with `replacement`
/// of the same length.
fn replace(bytes: &mut [u8], pattern: &[u8], replacement: &[u8]) {
    let at = bytes
        .windows(pattern.len())
        .position(|window| window == pattern)
        .expect("the pattern");
    bytes[at..at + pattern.len()].copy_from_slice(replacement);
}

/// The quic_transport_parameters extension of the client: its type and the
/// length of "client parameters".
const CLIENT_PARAMETERS: [u8; 4] = [0x00, 0x39, 0x00, 0x11];

#[test]
fn a_client_hello_without_tls_1_3_fails_with_protocol_version() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       _,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client
            && messages(bytes).first().map(|m| m.0) == Some(HandshakeType::CLIENT_HELLO)
        {
            // supported_versions with TLS 1.3 alone becomes TLS 1.2 alone.
            replace(
                bytes,
                &[0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04],
                &[0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x03],
            );
        }
    });
    let end = connection.server_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ServerAlertError::UnsupportedVersion)
    ));
    assert_eq!(alert_of(end), AlertDescription::PROTOCOL_VERSION);
}

#[test]
fn a_client_hello_without_transport_parameters_fails_with_missing_extension() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       _,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client
            && messages(bytes).first().map(|m| m.0) == Some(HandshakeType::CLIENT_HELLO)
        {
            // The extension becomes one of an unknown type.
            replace(bytes, &CLIENT_PARAMETERS, &[0x7a, 0x7a, 0x00, 0x11]);
        }
    });
    let end = connection.server_end;
    assert!(matches!(
        end.as_ref().and_then(|end| end.as_ref().err()?.failure()),
        Some(ServerAlertError::MissingExtension {
            extension: RequiredExtension::QuicTransportParameters
        })
    ));
    assert_eq!(alert_of(end), AlertDescription::MISSING_EXTENSION);
}

#[test]
fn a_client_hello_with_a_session_id_is_a_protocol_violation() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       _,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Client
            && messages(bytes).first().map(|m| m.0) == Some(HandshakeType::CLIENT_HELLO)
        {
            // The empty legacy_session_id, after the header, the version
            // and the random, becomes one byte long; the message grows by
            // one byte.
            assert_eq!(bytes[4 + 2 + 32], 0);
            bytes[4 + 2 + 32] = 1;
            bytes.insert(4 + 2 + 32 + 1, 0x5a);
            let len = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) + 1;
            bytes[1..4].copy_from_slice(&len.to_be_bytes()[1..]);
        }
    });
    match connection.server_end {
        Some(Err(error)) => {
            let Some(ProtocolViolationError::LegacySessionId { source }) =
                error.protocol_violation()
            else {
                panic!("not a protocol violation: {error:?}")
            };
            assert_eq!(source.session_id().as_bytes(), [0x5a]);
            assert!(error.alert().is_none() && error.transport().is_none());
        }
        _ => panic!("the server did not fail"),
    }
    assert!(connection.server_events.is_empty());
}

#[test]
fn a_ticket_with_another_early_data_size_is_a_protocol_violation() {
    let endpoints = Endpoints::new();
    let connection = connect!(&endpoints, Wire::new(), "example.com", |from,
                                                                       level,
                                                                       bytes: &mut Vec<
        u8,
    >| {
        if from == Sender::Server && level == CryptoLevel::OneRtt {
            // early_data is the last extension of each NewSessionTicket.
            for (found, range) in messages(bytes) {
                if found == HandshakeType::NEW_SESSION_TICKET {
                    assert_eq!(
                        &bytes[range.end - 8..range.end],
                        [0x00, 0x2a, 0x00, 0x04, 0xff, 0xff, 0xff, 0xff]
                    );
                    bytes[range.end - 4..range.end].copy_from_slice(&[0x00, 0x00, 0x40, 0x00]);
                }
            }
        }
    });
    assert!(matches!(connection.server_end, Some(Ok(()))));
    match connection.client_end {
        Some(Err(error)) => {
            let Some(ProtocolViolationError::MaxEarlyDataSize { source }) =
                error.protocol_violation()
            else {
                panic!("not a protocol violation: {error:?}")
            };
            assert_eq!(source.size().get(), 0x4000);
            assert!(error.alert().is_none());
        }
        _ => panic!("the client did not fail"),
    }
    // No ticket was kept: the next connection does a full handshake.
    let again = connect!(&endpoints, Wire::new(), "example.com", unchanged);
    let agreed = agreed(&again.server_events, &again.client_events);
    assert!(!agreed.resumed);
}

/// A client written out with the transcript chain, for a ClientHello the
/// client engine does not send: one that may leave out
/// psk_key_exchange_modes. It offers one suite, one group and Ed25519,
/// verifies the server as the engine does and writes its Finished; what
/// the server sends after that stays on the wire.
struct ScriptedClient {
    psk_modes: bool,
    step: ScriptedStep,
}

/// What a [`ScriptedClient`] waits for.
enum ScriptedStep {
    /// Nothing: its ClientHello is still to write.
    Start,
    /// The ServerHello, with the transcript and the started exchange.
    ServerHello(
        Transcript<Client, AwsLcBackend>,
        <AwsLcBackend as KeyExchange>::Started,
    ),
    /// EncryptedExtensions to the server's Finished.
    ServerFlight(HandshakeSecret<Client, AwsLcBackend>),
    /// Nothing: it wrote its Finished.
    Done,
}

impl ScriptedClient {
    fn new(psk_modes: bool) -> Self {
        Self {
            psk_modes,
            step: ScriptedStep::Start,
        }
    }

    fn poll(&mut self, wire: &mut Wire) -> Poll<ControlFlow<(), ()>> {
        let backend = AwsLcBackend;
        match core::mem::replace(&mut self.step, ScriptedStep::Done) {
            ScriptedStep::Start => {
                let group = backend.named_group(NamedGroup::X25519).unwrap();
                let exchange = KeyExchange::start(&backend, group).unwrap();
                let hello = self.hello(exchange.share().as_bytes());
                let mut encoded = Vec::new();
                write_client_hello(&mut encoded, &hello);
                wire.client.output.levels[index(CryptoLevel::Initial)].extend_from_slice(&encoded);
                let hash = backend.hash_algorithm(HashAlgorithm::Sha256).unwrap();
                let transcript = Transcript::new(&backend, hash).add(&encoded);
                self.step = ScriptedStep::ServerHello(transcript, exchange);
                Poll::Ready(ControlFlow::Continue(()))
            }
            ScriptedStep::ServerHello(transcript, exchange) => {
                let Some([server_hello]) =
                    take_messages(&mut wire.client.input, CryptoLevel::Initial)
                else {
                    self.step = ScriptedStep::ServerHello(transcript, exchange);
                    return Poll::Pending;
                };
                let ServerHelloMessage::ServerHello(reply) =
                    read_server_hello_message(&mut server_hello.as_slice()).unwrap()
                else {
                    panic!("a HelloRetryRequest for the one group offered");
                };
                let shared = exchange
                    .complete(reply.key_share().unwrap().key_exchange())
                    .unwrap();
                let (handshake, _) = transcript
                    .early_secret()
                    .server_hello(shared, &server_hello, None)
                    .unwrap();
                self.step = ScriptedStep::ServerFlight(handshake);
                Poll::Ready(ControlFlow::Continue(()))
            }
            ScriptedStep::ServerFlight(handshake) => {
                let Some(
                    [
                        encrypted_extensions,
                        certificate,
                        certificate_verify,
                        finished,
                    ],
                ) = take_messages(&mut wire.client.input, CryptoLevel::Handshake)
                else {
                    self.step = ScriptedStep::ServerFlight(handshake);
                    return Poll::Pending;
                };
                let Authentication::Certificate(awaiting) =
                    handshake.add(&encrypted_extensions).authentication()
                else {
                    panic!("resumed without offering a ticket");
                };
                let (main, _) = awaiting
                    .add(&certificate)
                    .verify_certificate_verify(&whole(&certificate_verify), leaf_key().key())
                    .unwrap()
                    .server_finished(&whole(&finished))
                    .unwrap();
                let (_, finished) = main.client_finished();
                wire.client.output.levels[index(CryptoLevel::Handshake)]
                    .extend_from_slice(finished.encoded());
                Poll::Ready(ControlFlow::Break(()))
            }
            ScriptedStep::Done => Poll::Pending,
        }
    }

    /// The ClientHello, with psk_key_exchange_modes naming psk_dhe_ke when
    /// `psk_modes`.
    fn hello(&self, share: &[u8]) -> ClientHello<Bytes> {
        let mut extensions = vec![
            ClientHelloExtension::ServerName(
                ServerName::new(Bytes::from_static(b"example.com")).unwrap(),
            ),
            ClientHelloExtension::SupportedVersions(
                CodePointList::new(vec![ProtocolVersion::TLS_1_3]).unwrap(),
            ),
            ClientHelloExtension::SupportedGroups(
                CodePointList::new(vec![NamedGroup::X25519]).unwrap(),
            ),
            ClientHelloExtension::SignatureAlgorithms(
                CodePointList::new(vec![SignatureScheme::ED25519]).unwrap(),
            ),
            ClientHelloExtension::ApplicationLayerProtocolNegotiation(
                ProtocolNameList::new(vec![h3()]).unwrap(),
            ),
            ClientHelloExtension::KeyShare(
                ClientShares::new(vec![
                    KeyShareEntry::new(NamedGroup::X25519, Bytes::copy_from_slice(share)).unwrap(),
                ])
                .unwrap(),
            ),
            ClientHelloExtension::QuicTransportParameters(
                OpaqueData::new(Bytes::from_static(b"client parameters")).unwrap(),
            ),
        ];
        if self.psk_modes {
            extensions.push(ClientHelloExtension::PskKeyExchangeModes(
                ByteCodePointList::new(vec![PskKeyExchangeMode::PSK_DHE_KE]).unwrap(),
            ));
        }
        ClientHello::new(
            ProtocolVersion::TLS_1_2,
            HelloRandom([7; 32]),
            LegacySessionId::EMPTY,
            CodePointList::new(vec![CipherSuite::TLS_AES_128_GCM_SHA256]).unwrap(),
            extensions,
        )
        .unwrap()
    }
}

/// Takes `N` whole messages from what `incoming` holds at `level`, or
/// nothing while fewer have arrived.
fn take_messages<const N: usize>(
    incoming: &mut Incoming,
    level: CryptoLevel,
) -> Option<[Vec<u8>; N]> {
    let buffer = &mut incoming.levels[index(level)];
    let bytes: Vec<u8> = buffer.iter().copied().collect();
    let mut taken = Vec::new();
    let mut end = 0;
    for (_, range) in messages(&bytes).into_iter().take(N) {
        let len = u32::from_be_bytes([
            0,
            bytes[range.start + 1],
            bytes[range.start + 2],
            bytes[range.start + 3],
        ]);
        if range.len() != 4 + len as usize {
            return None;
        }
        end = range.end;
        taken.push(bytes[range].to_vec());
    }
    let taken = taken.try_into().ok()?;
    buffer.drain(..end);
    Some(taken)
}

/// `bytes`, one whole message of the suite's hash, SHA-256.
fn whole(mut bytes: &[u8]) -> HandshakeMessage {
    bytes
        .sync_read_handshake_message_unpin(MessageLimits::default(), Some(HashAlgorithm::Sha256))
        .unwrap()
}

#[test]
fn a_client_that_does_not_name_psk_dhe_ke_gets_no_tickets() {
    // With psk_dhe_ke the same client gets the table's two tickets, so the
    // client is not what keeps them away.
    for (psk_modes, tickets) in [(true, 2), (false, 0)] {
        let endpoints = Endpoints::new();
        let mut wire = Wire::new();
        let mut server = pin!(LentCall::<ServerSide, _>::new(ServerSteps::new(
            server_handshake!(&endpoints),
            MessageLimits::default()
        )));
        let mut client = ScriptedClient::new(psk_modes);
        let run = run(
            &mut wire,
            |wire| poll_server!(server, wire),
            |wire| client.poll(wire),
            unchanged,
        );
        assert!(
            matches!(run.server_end, Some(Ok(()))),
            "psk_modes {psk_modes}"
        );
        assert!(run.client_end.is_some());
        assert!(matches!(
            run.server_events.last(),
            Some(ServerEvent::Completed(ServerHandshakeCompleted { .. }))
        ));
        assert_eq!(
            wire.seen(Sender::Server, HandshakeType::NEW_SESSION_TICKET),
            tickets,
            "psk_modes {psk_modes}"
        );
    }
}
