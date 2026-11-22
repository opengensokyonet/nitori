//! What a call or a hook hands the driver to change the structure.
//!
//! Requests are not defined yet: each kind of node will declare its own
//! and handle them on its own subtree, and what a call yields or a hook
//! returns will be the sum of every kind's requests with the address of
//! the node they are for. Until then the sum is [`NoRequest`], and the
//! hooks are written for any sum ([`RequestSum`]) so that their signatures
//! stay when it comes.
use core::fmt;

/// The sum of the requests a hook can return, for whatever structure the
/// driver has.
pub trait RequestSum: 'static {}

/// The sum of no requests: nothing changes the structure from inside yet.
pub enum NoRequest {}
impl RequestSum for NoRequest {}
impl fmt::Debug for NoRequest {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}
