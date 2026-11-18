//! A client's session tickets.
//!
//! A client keeps the tickets servers send, each with what resuming needs,
//! and takes one out when it connects to the same target again. A ticket
//! taken out is not handed out again: a client should not reuse a ticket
//! (RFC 9001 §4.5). A [`ClientTicketStore`] keeps them per
//! [`ServerTarget`], so a ticket only resumes a connection to the target
//! it came from; [`TakeOnceTicketMap`] keeps them in memory.

use crate::{
    cipher_suite::CipherSuite,
    client::ServerTarget,
    clock::{Clock, elapsed},
    extension::alpn::ProtocolName,
    message::new_session_ticket::TicketLifetime,
    schedule::ResumptionPsk,
    verify::SubjectPublicKey,
};
use bytes::Bytes;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, PoisonError},
    time::Duration,
};
use time::UtcDateTime;

/// A ticket a client received, with what it needs to resume with it.
pub struct StoredTicket {
    /// The ticket, the identity to offer.
    pub ticket: Bytes,
    /// The PSK of the ticket.
    pub psk: ResumptionPsk,
    /// The cipher suite of the connection the ticket came from; its hash
    /// must be the hash of the resumed connection's suite.
    pub suite: CipherSuite,
    /// The `ticket_age_add` of the ticket.
    pub ticket_age_add: u32,
    /// How long the server said the ticket is valid.
    pub lifetime: TicketLifetime,
    /// When the client received the ticket.
    pub received_at: UtcDateTime,
    /// Whether the ticket allows 0-RTT.
    pub early_data: bool,
    /// The application protocol of the connection the ticket came from.
    pub application_protocol: Option<ProtocolName<Bytes>>,
    /// The server's transport parameters of that connection, which 0-RTT
    /// remembers (RFC 9000 §7.4.1).
    pub transport_parameters: Bytes,
    /// The public key the server authenticated with on that connection,
    /// which a connection resumed with the ticket keeps as the server's
    /// identity.
    pub identity: SubjectPublicKey,
}

impl StoredTicket {
    /// The obfuscated age to offer at `now`: the age in milliseconds plus
    /// `ticket_age_add`, modulo 2<sup>32</sup> (RFC 9846 §4.3.11.1).
    ///
    /// A `now` before the ticket was received counts as an age of zero.
    pub fn obfuscated_ticket_age(&self, now: UtcDateTime) -> u32 {
        let age = elapsed(self.received_at, now).unwrap_or_default();
        // The age is taken modulo 2^32 milliseconds, as the field holds it.
        (age.as_millis() as u32).wrapping_add(self.ticket_age_add)
    }

    fn usable_at(&self, now: UtcDateTime, max_age: Duration) -> bool {
        let lifetime = Duration::from_secs(u64::from(self.lifetime.seconds())).min(max_age);
        elapsed(self.received_at, now).is_none_or(|age| age < lifetime)
    }
}

impl core::fmt::Debug for StoredTicket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StoredTicket")
            .field("suite", &self.suite)
            .field("lifetime", &self.lifetime)
            .field("received_at", &self.received_at)
            .field("early_data", &self.early_data)
            .finish_non_exhaustive()
    }
}

/// Where a client keeps its tickets, per target.
pub trait ClientTicketStore {
    /// Takes a ticket for `target` out of the store, or `None` when it has
    /// no usable one. The store does not hand the same ticket out again.
    fn take(&self, target: &ServerTarget) -> Option<StoredTicket>;

    /// Keeps `ticket`, received from `target`.
    fn store(&self, target: &ServerTarget, ticket: StoredTicket);
}

impl<S: ClientTicketStore + ?Sized> ClientTicketStore for &S {
    fn take(&self, target: &ServerTarget) -> Option<StoredTicket> {
        (**self).take(target)
    }

    fn store(&self, target: &ServerTarget, ticket: StoredTicket) {
        (**self).store(target, ticket);
    }
}

impl<S: ClientTicketStore + ?Sized> ClientTicketStore for std::sync::Arc<S> {
    fn take(&self, target: &ServerTarget) -> Option<StoredTicket> {
        (**self).take(target)
    }

    fn store(&self, target: &ServerTarget, ticket: StoredTicket) {
        (**self).store(target, ticket);
    }
}

/// Tickets in memory, a queue per target, each handed out once.
///
/// `take` hands out the newest ticket of the target that is still usable:
/// younger than both its lifetime and the store's own maximum age, by the
/// clock `K`; the ones that are not are dropped on the way. Each target
/// keeps at most a fixed number of tickets, the oldest going first, and
/// the store keeps at most [`TakeOnceTicketMap::TARGET_CAPACITY`] targets,
/// a new one replacing an arbitrary one past it.
pub struct TakeOnceTicketMap<K> {
    clock: K,
    max_age: Duration,
    per_target: usize,
    tickets: Mutex<HashMap<ServerTarget, VecDeque<StoredTicket>>>,
}

impl<K> TakeOnceTicketMap<K> {
    /// The most targets the store keeps tickets for.
    pub const TARGET_CAPACITY: usize = 1024;

    /// A store that hands out tickets younger than `max_age` and keeps at
    /// most `per_target` of them for each target.
    pub fn new(clock: K, max_age: Duration, per_target: usize) -> Self {
        Self {
            clock,
            max_age,
            per_target,
            tickets: Mutex::default(),
        }
    }
}

impl<K: Clock> ClientTicketStore for TakeOnceTicketMap<K> {
    fn take(&self, target: &ServerTarget) -> Option<StoredTicket> {
        let now = self.clock.now();
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        let queue = tickets.get_mut(target)?;
        let found = core::iter::from_fn(|| queue.pop_back())
            .find(|ticket| ticket.usable_at(now, self.max_age));
        if queue.is_empty() {
            tickets.remove(target);
        }
        found
    }

    fn store(&self, target: &ServerTarget, ticket: StoredTicket) {
        if self.per_target == 0 {
            return;
        }
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        if tickets.len() >= Self::TARGET_CAPACITY
            && !tickets.contains_key(target)
            && let Some(evicted) = tickets.keys().next().cloned()
        {
            tickets.remove(&evicted);
        }
        let queue = tickets.entry(target.clone()).or_default();
        if queue.len() >= self.per_target {
            queue.pop_front();
        }
        queue.push_back(ticket);
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientTicketStore, StoredTicket, TakeOnceTicketMap};
    use crate::{
        cipher_suite::CipherSuite, client::ServerTarget, clock::ManualClock,
        message::new_session_ticket::TicketLifetime, schedule::ResumptionPsk,
        verify::SubjectPublicKey,
    };
    use bytes::Bytes;
    use nitori_crypto::Secret;
    use std::time::Duration;
    use time::UtcDateTime;

    /// An Ed25519 SubjectPublicKeyInfo.
    const ED25519_SPKI: &[u8] = &[
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 1, 2, 3, 4, 5, 6,
        7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
        30, 31, 32,
    ];

    fn start() -> UtcDateTime {
        UtcDateTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn ticket(byte: u8, received_at: UtcDateTime) -> StoredTicket {
        StoredTicket {
            ticket: Bytes::from(vec![byte; 8]),
            psk: ResumptionPsk::new(Secret::new(&[byte; 32]).unwrap()),
            suite: CipherSuite::TLS_AES_128_GCM_SHA256,
            ticket_age_add: 0xffff_ff00,
            lifetime: TicketLifetime::new(600).unwrap(),
            received_at,
            early_data: false,
            application_protocol: None,
            transport_parameters: Bytes::new(),
            identity: SubjectPublicKey::from_spki(ED25519_SPKI).unwrap(),
        }
    }

    fn target(name: &str) -> ServerTarget {
        ServerTarget::from_host_name(name).unwrap()
    }

    #[test]
    fn a_ticket_is_taken_once_and_only_for_its_target() {
        let store = TakeOnceTicketMap::new(ManualClock::new(start()), Duration::from_secs(3600), 4);
        store.store(&target("example.com"), ticket(1, start()));
        store.store(&target("example.com"), ticket(2, start()));
        assert!(store.take(&target("example.net")).is_none());
        assert_eq!(store.take(&target("Example.com")).unwrap().ticket[0], 2);
        assert_eq!(store.take(&target("example.com")).unwrap().ticket[0], 1);
        assert!(store.take(&target("example.com")).is_none());
    }

    #[test]
    fn a_ticket_past_its_lifetime_or_the_maximum_age_is_dropped() {
        let clock = ManualClock::new(start());
        let store = TakeOnceTicketMap::new(&clock, Duration::from_secs(300), 4);
        store.store(&target("example.com"), ticket(1, start()));
        clock.advance(Duration::from_secs(300));
        assert!(store.take(&target("example.com")).is_none());
        let long = TakeOnceTicketMap::new(&clock, Duration::from_secs(3600), 4);
        long.store(&target("example.com"), ticket(1, start()));
        long.store(
            &target("example.com"),
            ticket(2, start() + Duration::from_secs(250)),
        );
        clock.advance(Duration::from_secs(400));
        // The newer ticket, received 450 s ago, is within its 600 s.
        assert_eq!(long.take(&target("example.com")).unwrap().ticket[0], 2);
        assert!(long.take(&target("example.com")).is_none());
    }

    #[test]
    fn each_target_keeps_its_newest_tickets() {
        let store = TakeOnceTicketMap::new(ManualClock::new(start()), Duration::from_secs(3600), 2);
        for byte in 1..=3 {
            store.store(&target("example.com"), ticket(byte, start()));
        }
        assert_eq!(store.take(&target("example.com")).unwrap().ticket[0], 3);
        assert_eq!(store.take(&target("example.com")).unwrap().ticket[0], 2);
        assert!(store.take(&target("example.com")).is_none());
    }

    #[test]
    fn the_obfuscated_ticket_age_adds_ticket_age_add() {
        let ticket = ticket(1, start());
        assert_eq!(ticket.obfuscated_ticket_age(start()), 0xffff_ff00);
        assert_eq!(
            ticket.obfuscated_ticket_age(start() + Duration::from_millis(0x101)),
            0x0000_0001
        );
        assert_eq!(
            ticket.obfuscated_ticket_age(start() - Duration::from_secs(1)),
            0xffff_ff00
        );
    }
}
