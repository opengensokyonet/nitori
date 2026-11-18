//! How a server issues session tickets and opens the ones clients offer.
//!
//! A [`TicketPolicy`] decides both. Issuing is one interaction per
//! connection: after the handshake, [`TicketPolicy::issuing`] begins an
//! [`Issuing`] for the session, which yields a [`TicketSlot`] for each
//! ticket to send, and the handshake seals each slot with the ticket's PSK
//! into the [`Ticket`] it sends. The handshake generates the nonce and
//! `ticket_age_add` of each ticket. Opening is one interaction per
//! ClientHello that offers PSKs: [`TicketPolicy::open`] finds the ticket an
//! identity names and returns it [`Opened`], with its PSK and the
//! [`TicketState`] it recorded.
//!
//! The handshake checks the binder, the ticket age, and that version,
//! suite and application protocol match; the connection checks that the
//! transport parameters are compatible. A policy that admits 0-RTT with
//! [`Opened::admit_early_data`] gives one necessary condition of several.
//! RFC 9846 §8 requires each server instance to accept a 0-RTT handshake at
//! most once, which the policy ensures there. Dropping an [`Opened`] means
//! nothing to the handshake: when a ticket counts as used is the policy's
//! decision.
//!
//! [`ZeroRttOnceTicketTable`] keeps tickets in memory, where the ticket is
//! the key of its entry, and spends a ticket when it admits 0-RTT with it.

use crate::{
    cipher_suite::CipherSuite,
    clock::{Clock, elapsed},
    extension::{alpn::ProtocolName, pre_shared_key::OfferedPsks, pre_shared_key::PskIndex},
    message::new_session_ticket::{TicketLifetime, TicketNonce},
    schedule::ResumptionPsk,
    vector::LengthOutOfRangeError,
    width::Width,
};
use bytes::Bytes;
use core::future::{Future, Ready, ready};
use nitori_crypto::{Digest, Random, RandomError, Secret};
use snafu::{ResultExt, Snafu};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use time::UtcDateTime;

/// What a resumed connection must find again, as the handshake knows it
/// when it issues tickets (RFC 9846 §4.7.1, RFC 9000 §7.4.1,
/// RFC 9001 §4.6.3).
///
/// `T` is the type of the server's selected target, which carries the
/// application's configuration. Each ticket adds its own PSK,
/// `ticket_age_add` and time of issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resumable<T> {
    /// The cipher suite of the connection.
    pub suite: CipherSuite,
    /// The negotiated application protocol.
    pub application_protocol: Option<ProtocolName<Bytes>>,
    /// The selected target.
    pub target: T,
    /// The server's own transport parameters, as it sent them.
    pub transport_parameters: Bytes,
}

/// A ticket to send in a NewSessionTicket.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ticket {
    lifetime: TicketLifetime,
    ticket: Bytes,
    early_data: bool,
}

impl Ticket {
    /// The ticket `ticket`, valid for `lifetime`, which allows 0-RTT when
    /// `early_data`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] for an empty ticket or one above 65535 bytes.
    pub fn new(
        lifetime: TicketLifetime,
        ticket: Bytes,
        early_data: bool,
    ) -> Result<Self, LengthOutOfRangeError> {
        LengthOutOfRangeError::check(ticket.len(), 1..=Width::U16.max())?;
        Ok(Self {
            lifetime,
            ticket,
            early_data,
        })
    }

    /// How long the ticket is valid.
    pub fn lifetime(&self) -> TicketLifetime {
        self.lifetime
    }

    /// The ticket, the identity a client offers to resume.
    pub fn ticket(&self) -> &Bytes {
        &self.ticket
    }

    /// Whether the ticket allows 0-RTT, which the NewSessionTicket states
    /// with `early_data`.
    pub fn early_data(&self) -> bool {
        self.early_data
    }
}

/// What a policy recorded of a ticket, found again when a client offers
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketState<T> {
    /// The session the ticket resumes.
    pub resumable: Resumable<T>,
    /// The `ticket_age_add` of the ticket, to recover its age from the
    /// obfuscated age a client sends.
    pub ticket_age_add: u32,
    /// When the ticket was issued.
    pub issued_at: UtcDateTime,
    /// How long the ticket is valid.
    pub lifetime: TicketLifetime,
    /// Whether the ticket allows 0-RTT.
    pub early_data: bool,
}

/// What identifies a ClientHello for anti-replay: a hash the handshake
/// computes over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientHelloKey(Digest);

impl ClientHelloKey {
    /// The key `hash`.
    pub fn new(hash: Digest) -> Self {
        Self(hash)
    }

    /// The hash.
    pub fn hash(&self) -> &Digest {
        &self.0
    }
}

/// How a server issues tickets and opens offered ones.
///
/// `T` is the type of the server's selected target.
pub trait TicketPolicy<T> {
    /// The issuing of tickets on one connection.
    type Issuing: Issuing;

    /// A ticket the policy recognized.
    type Opened: Opened<T>;

    /// The opening of the PSKs of one ClientHello.
    type Opening<'a, B: 'a>: Future<Output = Option<(PskIndex, Self::Opened)>> + 'a
    where
        Self: 'a;

    /// Begins issuing tickets for `session`.
    fn issuing(&self, session: &Resumable<T>) -> Self::Issuing;

    /// Finds a ticket among `offered`: the index of the identity that names
    /// it, with the ticket opened, or `None` when no identity names a
    /// ticket the policy accepts, for example one that expired.
    fn open<'a, B: AsRef<[u8]> + 'a>(&'a self, offered: &'a OfferedPsks<B>)
    -> Self::Opening<'a, B>;
}

/// The issuing of tickets on one connection.
pub trait Issuing {
    /// A ticket to seal.
    type Slot: TicketSlot;

    /// Waiting for the next ticket.
    type Next<'a>: Future<Output = Option<Self::Slot>> + 'a
    where
        Self: 'a;

    /// The next ticket to send, or `None` when the policy sends no more on
    /// this connection.
    fn next(&mut self) -> Self::Next<'_>;
}

/// One ticket to send, waiting for its secret.
pub trait TicketSlot {
    /// A failure to seal.
    type Error;

    /// Sealing in progress; it holds what it needs of the arguments.
    type Sealing: Future<Output = Result<Ticket, Self::Error>>;

    /// Seals `psk`, the PSK of the ticket with `nonce`, with the ticket's
    /// `ticket_age_add`: for a stateless ticket, encrypts them into the
    /// ticket; for a stateful one, stores them under the ticket.
    fn seal<N: AsRef<[u8]>>(
        self,
        psk: ResumptionPsk,
        nonce: &TicketNonce<N>,
        ticket_age_add: u32,
    ) -> Self::Sealing;
}

/// A ticket a policy recognized in a ClientHello.
pub trait Opened<T> {
    /// Deciding on 0-RTT.
    type Admitting: Future<Output = bool>;

    /// The PSK of the ticket.
    fn psk(&self) -> &ResumptionPsk;

    /// What the policy recorded of the ticket.
    fn state(&self) -> &TicketState<T>;

    /// Whether the policy lets 0-RTT in with this ticket for the
    /// ClientHello `hello`. A `true` from the same server instance comes at
    /// most once for one 0-RTT handshake (RFC 9846 §8).
    fn admit_early_data(self, hello: &ClientHelloKey) -> Self::Admitting;
}

/// The length of the tickets of a [`ZeroRttOnceTicketTable`].
const TICKET_LEN: usize = 16;

/// Tickets kept in memory, each a random key of its entry, and spent when
/// they admit 0-RTT.
///
/// Each connection gets a fixed number of tickets with a fixed lifetime.
/// Opening finds the first offered identity that names an entry that has
/// not expired; it does not spend the ticket, so a ticket resumes without
/// 0-RTT until it expires. Admitting 0-RTT removes the entry, so that a
/// ticket admits 0-RTT once at most; it admits only when the ticket allows
/// 0-RTT. Clones share the table, which holds at most a fixed number of
/// entries: past it, sealing fails once the expired ones are gone.
///
/// `R` generates the tickets and `K` tells the time.
pub struct ZeroRttOnceTicketTable<T, R, K> {
    entries: Arc<Mutex<HashMap<[u8; TICKET_LEN], Entry<T>>>>,
    random: R,
    clock: K,
    settings: TableSettings,
}

/// The fixed choices of a [`ZeroRttOnceTicketTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TableSettings {
    tickets_per_connection: usize,
    lifetime: TicketLifetime,
    early_data: bool,
    capacity: usize,
}

struct Entry<T> {
    psk: StoredPsk,
    state: TicketState<T>,
}

/// The PSK of an entry, kept as a [`Secret`], which a [`ResumptionPsk`]
/// does not give up.
struct StoredPsk(Secret);

impl From<ResumptionPsk> for StoredPsk {
    fn from(psk: ResumptionPsk) -> Self {
        // invariant: the bytes come from a `Secret`.
        Self(Secret::new(psk.secret().as_bytes()).expect("a PSK fits a secret"))
    }
}

impl StoredPsk {
    fn copy(&self) -> ResumptionPsk {
        // invariant: the bytes come from a `Secret`.
        ResumptionPsk::new(Secret::new(self.0.as_bytes()).expect("a PSK fits a secret"))
    }
}

impl<T> Entry<T> {
    fn expired(&self, now: UtcDateTime) -> bool {
        let lifetime = Duration::from_secs(u64::from(self.state.lifetime.seconds()));
        elapsed(self.state.issued_at, now).is_some_and(|age| age >= lifetime)
    }
}

impl<T, R, K> ZeroRttOnceTicketTable<T, R, K> {
    /// A table that issues `tickets_per_connection` tickets valid for
    /// `lifetime`, which allow 0-RTT when `early_data`, and holds at most
    /// `capacity` of them.
    pub fn new(
        random: R,
        clock: K,
        tickets_per_connection: usize,
        lifetime: TicketLifetime,
        early_data: bool,
        capacity: usize,
    ) -> Self {
        Self {
            entries: Arc::default(),
            random,
            clock,
            settings: TableSettings {
                tickets_per_connection,
                lifetime,
                early_data,
                capacity,
            },
        }
    }

    /// The number of tickets the table holds, expired ones included until
    /// they are found or swept.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether the table holds no ticket.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T, R: Clone, K: Clone> Clone for ZeroRttOnceTicketTable<T, R, K> {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
            random: self.random.clone(),
            clock: self.clock.clone(),
            settings: self.settings,
        }
    }
}

impl<T: Clone, R: Random + Clone, K: Clock + Clone> TicketPolicy<T>
    for ZeroRttOnceTicketTable<T, R, K>
{
    type Issuing = TableIssuing<T, R, K>;
    type Opened = TableOpened<T>;
    type Opening<'a, B: 'a>
        = Ready<Option<(PskIndex, TableOpened<T>)>>
    where
        Self: 'a;

    fn issuing(&self, session: &Resumable<T>) -> TableIssuing<T, R, K> {
        TableIssuing {
            table: self.clone(),
            resumable: session.clone(),
            remaining: self.settings.tickets_per_connection,
        }
    }

    fn open<'a, B: AsRef<[u8]> + 'a>(
        &'a self,
        offered: &'a OfferedPsks<B>,
    ) -> Self::Opening<'a, B> {
        let now = self.clock.now();
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let found = offered
            .identities()
            .iter()
            .enumerate()
            .find_map(|(index, identity)| {
                let key = <[u8; TICKET_LEN]>::try_from(identity.identity().as_ref()).ok()?;
                let entry = entries.get(&key)?;
                if entry.expired(now) {
                    entries.remove(&key);
                    return None;
                }
                // invariant: `PreSharedKey` holds at most 65535 bytes of
                // identities, so the index fits in `u16`.
                let index = PskIndex::new(u16::try_from(index).expect("index of an identity"));
                let opened = TableOpened {
                    entries: Arc::clone(&self.entries),
                    key,
                    psk: entry.psk.copy(),
                    state: entry.state.clone(),
                };
                Some((index, opened))
            });
        ready(found)
    }
}

/// The issuing of a [`ZeroRttOnceTicketTable`] on one connection.
pub struct TableIssuing<T, R, K> {
    table: ZeroRttOnceTicketTable<T, R, K>,
    resumable: Resumable<T>,
    remaining: usize,
}

impl<T: Clone, R: Random + Clone, K: Clock + Clone> Issuing for TableIssuing<T, R, K> {
    type Slot = TableSlot<T, R, K>;
    type Next<'a>
        = Ready<Option<TableSlot<T, R, K>>>
    where
        Self: 'a;

    fn next(&mut self) -> Self::Next<'_> {
        if self.remaining == 0 {
            return ready(None);
        }
        self.remaining -= 1;
        ready(Some(TableSlot {
            table: self.table.clone(),
            resumable: self.resumable.clone(),
        }))
    }
}

/// One ticket of a [`ZeroRttOnceTicketTable`], stored when it is sealed.
pub struct TableSlot<T, R, K> {
    table: ZeroRttOnceTicketTable<T, R, K>,
    resumable: Resumable<T>,
}

impl<T, R: Random, K: Clock> TicketSlot for TableSlot<T, R, K> {
    type Error = SealTicketError;
    type Sealing = Ready<Result<Ticket, SealTicketError>>;

    fn seal<N: AsRef<[u8]>>(
        self,
        psk: ResumptionPsk,
        _nonce: &TicketNonce<N>,
        ticket_age_add: u32,
    ) -> Self::Sealing {
        ready(self.store(psk, ticket_age_add))
    }
}

impl<T, R: Random, K: Clock> TableSlot<T, R, K> {
    fn store(self, psk: ResumptionPsk, ticket_age_add: u32) -> Result<Ticket, SealTicketError> {
        let key: [u8; TICKET_LEN] = self
            .table
            .random
            .random()
            .context(seal_ticket_error::RandomSnafu)?;
        let settings = self.table.settings;
        let now = self.table.clock.now();
        let mut entries = self
            .table
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= settings.capacity {
            entries.retain(|_, entry| !entry.expired(now));
        }
        if entries.len() >= settings.capacity {
            return seal_ticket_error::TableFullSnafu.fail();
        }
        let state = TicketState {
            resumable: self.resumable,
            ticket_age_add,
            issued_at: now,
            lifetime: settings.lifetime,
            early_data: settings.early_data,
        };
        entries.insert(
            key,
            Entry {
                psk: psk.into(),
                state,
            },
        );
        // invariant: the key is 16 bytes, within 1..=65535.
        Ok(Ticket::new(
            settings.lifetime,
            Bytes::copy_from_slice(&key),
            settings.early_data,
        )
        .expect("a 16-byte ticket"))
    }
}

/// A ticket of a [`ZeroRttOnceTicketTable`] could not be stored.
#[derive(Debug, Snafu)]
#[snafu(module(seal_ticket_error), visibility(pub(crate)))]
pub enum SealTicketError {
    /// The ticket could not be generated.
    #[snafu(display("ticket generation failed"))]
    Random {
        /// The failure of the random source.
        source: RandomError,
    },
    /// The table holds as many unexpired tickets as it may.
    #[snafu(display("ticket table full"))]
    TableFull,
}

/// A ticket of a [`ZeroRttOnceTicketTable`] that a ClientHello offered.
pub struct TableOpened<T> {
    entries: Arc<Mutex<HashMap<[u8; TICKET_LEN], Entry<T>>>>,
    key: [u8; TICKET_LEN],
    psk: ResumptionPsk,
    state: TicketState<T>,
}

impl<T> Opened<T> for TableOpened<T> {
    type Admitting = Ready<bool>;

    fn psk(&self) -> &ResumptionPsk {
        &self.psk
    }

    fn state(&self) -> &TicketState<T> {
        &self.state
    }

    fn admit_early_data(self, _hello: &ClientHelloKey) -> Ready<bool> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let spent = entries.remove(&self.key).is_some();
        ready(spent && self.state.early_data)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientHelloKey, Issuing, Opened, Resumable, SealTicketError, TicketPolicy, TicketSlot,
        ZeroRttOnceTicketTable,
    };
    use crate::{
        cipher_suite::CipherSuite,
        clock::ManualClock,
        extension::pre_shared_key::{OfferedPsks, PskBinderEntry, PskIdentity, PskIndex},
        message::new_session_ticket::{TicketLifetime, TicketNonce},
        schedule::ResumptionPsk,
        test_support::ready_now as now,
    };
    use bytes::Bytes;
    use nitori_crypto::{Digest, HashAlgorithm, Secret};
    use nitori_crypto_aws_lc::AwsLcBackend;
    use std::{sync::Arc, time::Duration};
    use time::UtcDateTime;

    type Table = ZeroRttOnceTicketTable<&'static str, AwsLcBackend, Arc<ManualClock>>;

    fn table(clock: &Arc<ManualClock>, early_data: bool, capacity: usize) -> Table {
        ZeroRttOnceTicketTable::new(
            AwsLcBackend,
            Arc::clone(clock),
            2,
            TicketLifetime::new(3600).unwrap(),
            early_data,
            capacity,
        )
    }

    fn clock() -> Arc<ManualClock> {
        Arc::new(ManualClock::new(
            UtcDateTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        ))
    }

    fn session() -> Resumable<&'static str> {
        Resumable {
            suite: CipherSuite::TLS_AES_128_GCM_SHA256,
            application_protocol: None,
            target: "example.com",
            transport_parameters: Bytes::from_static(&[1, 2, 3]),
        }
    }

    fn psk(byte: u8) -> ResumptionPsk {
        ResumptionPsk::new(Secret::new(&[byte; 32]).unwrap())
    }

    fn offered(identities: &[&[u8]]) -> OfferedPsks<Vec<u8>> {
        OfferedPsks::new(
            identities
                .iter()
                .map(|identity| PskIdentity::new(identity.to_vec(), 0).unwrap())
                .collect(),
            identities
                .iter()
                .map(|_| PskBinderEntry::new(vec![0; 32]).unwrap())
                .collect(),
        )
        .unwrap()
    }

    fn hello() -> ClientHelloKey {
        ClientHelloKey::new(Digest::new(HashAlgorithm::Sha256, &[7; 32]).unwrap())
    }

    fn issue(table: &Table, byte: u8) -> Bytes {
        let mut issuing = table.issuing(&session());
        let slot = now(issuing.next()).unwrap();
        let nonce = TicketNonce::new([byte]).unwrap();
        now(slot.seal(psk(byte), &nonce, 0x0102_0304))
            .unwrap()
            .ticket()
            .clone()
    }

    #[test]
    fn each_connection_gets_the_configured_number_of_tickets() {
        let table = table(&clock(), true, 16);
        let mut issuing = table.issuing(&session());
        let first = now(issuing.next()).unwrap();
        let second = now(issuing.next()).unwrap();
        assert!(now(issuing.next()).is_none());
        let nonce = TicketNonce::new([0]).unwrap();
        let a = now(first.seal(psk(1), &nonce, 1)).unwrap();
        let b = now(second.seal(psk(2), &nonce, 2)).unwrap();
        assert_ne!(a.ticket(), b.ticket());
        assert_eq!(a.ticket().len(), 16);
        assert_eq!(a.lifetime().seconds(), 3600);
        assert!(a.early_data());
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn an_offered_ticket_opens_with_its_psk_and_state() {
        let clock = clock();
        let table = table(&clock, true, 16);
        let ticket = issue(&table, 9);
        let (index, opened) = now(table.open(&offered(&[b"unknown", &ticket]))).unwrap();
        assert_eq!(index, PskIndex::new(1));
        assert_eq!(opened.psk().secret().as_bytes(), &[9; 32]);
        let state = opened.state();
        assert_eq!(state.resumable, session());
        assert_eq!(state.ticket_age_add, 0x0102_0304);
        assert_eq!(state.issued_at, crate::clock::Clock::now(&clock));
        assert!(now(table.open(&offered(&[b"unknown"]))).is_none());
    }

    #[test]
    fn a_ticket_admits_early_data_once() {
        let table = table(&clock(), true, 16);
        let ticket = issue(&table, 1);
        let (_, first) = now(table.open(&offered(&[&ticket]))).unwrap();
        let (_, second) = now(table.open(&offered(&[&ticket]))).unwrap();
        assert!(now(first.admit_early_data(&hello())));
        assert!(!now(second.admit_early_data(&hello())));
        // Spent, the ticket no longer resumes.
        assert!(now(table.open(&offered(&[&ticket]))).is_none());
    }

    #[test]
    fn resumption_alone_does_not_spend_a_ticket() {
        let table = table(&clock(), true, 16);
        let ticket = issue(&table, 1);
        drop(now(table.open(&offered(&[&ticket]))).unwrap());
        assert!(now(table.open(&offered(&[&ticket]))).is_some());
    }

    #[test]
    fn a_ticket_without_early_data_does_not_admit_it() {
        let table = table(&clock(), false, 16);
        let ticket = issue(&table, 1);
        let (_, opened) = now(table.open(&offered(&[&ticket]))).unwrap();
        assert!(!now(opened.admit_early_data(&hello())));
    }

    #[test]
    fn an_expired_ticket_does_not_open() {
        let clock = clock();
        let table = table(&clock, true, 16);
        let ticket = issue(&table, 1);
        clock.advance(Duration::from_secs(3599));
        assert!(now(table.open(&offered(&[&ticket]))).is_some());
        clock.advance(Duration::from_secs(1));
        assert!(now(table.open(&offered(&[&ticket]))).is_none());
        assert!(table.is_empty());
    }

    #[test]
    fn a_full_table_refuses_until_tickets_expire() {
        let clock = clock();
        let table = table(&clock, true, 1);
        issue(&table, 1);
        let slot = now(table.issuing(&session()).next()).unwrap();
        let nonce = TicketNonce::new([0]).unwrap();
        assert!(matches!(
            now(slot.seal(psk(2), &nonce, 0)),
            Err(SealTicketError::TableFull)
        ));
        clock.advance(Duration::from_secs(3600));
        issue(&table, 3);
        assert_eq!(table.len(), 1);
    }
}
