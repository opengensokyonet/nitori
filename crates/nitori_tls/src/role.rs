//! The two roles of a handshake, as type parameters.
//!
//! The transcript and key schedule of [`schedule`](crate::schedule) serve
//! both roles with one chain of types, parameterized by a role that is
//! [`Server`] or [`Client`]. The steps where the roles differ, the order of
//! the Finished messages, PSK binders and the direction of 0-RTT, are
//! methods of one role only.

use crate::schedule::TrafficSecret;

mod sealed {
    pub trait Sealed {
        /// Orders a pair given as (client, server) into (sent, received) by
        /// this role.
        fn send_receive<T>(client: T, server: T) -> (T, T);
    }
}

/// A role of a handshake: implemented by [`Server`] and [`Client`] alone.
pub trait HandshakeRole: sealed::Sealed + 'static {
    /// What the role holds back from the Main Secret step until the
    /// handshake completes.
    ///
    /// A server must not process 1-RTT packets before the handshake completes
    /// (RFC 9001 §5.7), so it holds the client's 1-RTT traffic secret until
    /// it has verified the client's Finished. A client has both directions
    /// once it has verified the server's Finished, and holds nothing.
    type Withheld;
}

/// The server role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Server;

/// The client role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Client;

impl sealed::Sealed for Server {
    fn send_receive<T>(client: T, server: T) -> (T, T) {
        (server, client)
    }
}

impl sealed::Sealed for Client {
    fn send_receive<T>(client: T, server: T) -> (T, T) {
        (client, server)
    }
}

impl HandshakeRole for Server {
    type Withheld = TrafficSecret;
}

impl HandshakeRole for Client {
    type Withheld = ();
}
