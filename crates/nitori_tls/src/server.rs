//! The capabilities a server's handshake consults, with implementations
//! for plain Web connections, and the server's handshake engine.
//!
//! Each capability is a trait the server's handshake takes as a parameter:
//!
//! - [`layers`]: the encrypted layers of a ClientHello, opened before the
//!   handshake handles it.
//! - [`choice`]: which cipher suite and key exchange group to use, from what
//!   the ClientHello offers.
//! - [`targets`]: which target the ClientHello asks for, and its
//!   application protocol.
//! - [`identity`]: the certificate chain a server presents, and its
//!   signature.
//! - [`peer`]: whether to ask the client for a certificate.
//! - [`ticket`]: which session tickets to issue, and which offered ones to
//!   accept.
//!
//! The handshake checks what a capability returns against the protocol, so
//! an implementation decides within the rules and does not enforce them.
//!
//! [`engine`] is the server's handshake engine, which takes these
//! capabilities.

pub mod choice;
pub mod engine;
pub mod identity;
pub mod layers;
pub mod peer;
pub mod targets;
pub mod ticket;
