//! The layers of a ClientHello a server opens before it handles it.
//!
//! A ClientHello can carry encrypted layers, such as an inner ClientHello
//! of ECH. [`HelloLayers::open`] takes the ClientHello as received and
//! hands over the one the handshake continues with, as an
//! [`AcceptedHello`].
//!
//! After a HelloRetryRequest the second ClientHello is opened with what
//! opening the first gave: its outer ClientHello is opened with the HPKE
//! context of the first, and its payload is not opened when the first
//! rejected ECH (RFC 9849 §7.1.1). That state lives in the first
//! [`AcceptedHello`], which the handshake hands to the second
//! [`HelloLayers::open`], so the layers keep no state of a connection.
//!
//! [`PassThroughLayers`] opens nothing and hands the ClientHello over as it
//! came.
//!
//! [`PreopenedLayers`] hands over a first ClientHello opened before the
//! handshake began, such as by a server that decided what to do with the
//! connection, and opens a second one with the layers it wraps.

use crate::message::{HandshakeMessage, client_hello::ClientHello};
use bytes::Bytes;
use core::{
    convert::Infallible,
    future::{Future, Ready, ready},
};
use std::sync::{Mutex, PoisonError};

/// The ClientHello a handshake continues with, after its layers.
pub trait AcceptedHello {
    /// The encoding of the ClientHello, which goes into the transcript.
    fn encoded(&self) -> &HandshakeMessage;

    /// The ClientHello.
    fn client_hello(&self) -> &ClientHello<Bytes>;
}

/// How a server opens the layers of a ClientHello.
pub trait HelloLayers {
    /// The ClientHello after its layers.
    type Accepted: AcceptedHello;

    /// A failure to open.
    type Error;

    /// Opening in progress.
    type Opening<'a>: Future<Output = Result<Self::Accepted, Self::Error>> + 'a
    where
        Self: 'a;

    /// Opens `hello`, received as `encoded`. `first` is `None` for the
    /// first ClientHello of a connection, and for the second, sent after a
    /// HelloRetryRequest, what opening the first gave.
    fn open<'a>(
        &'a self,
        encoded: HandshakeMessage,
        hello: ClientHello<Bytes>,
        first: Option<&'a Self::Accepted>,
    ) -> Self::Opening<'a>;
}

/// Layers that open nothing: the ClientHello is handed over as it came.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PassThroughLayers;

/// A ClientHello handed over as it came, by [`PassThroughLayers`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassThroughHello {
    encoded: HandshakeMessage,
    hello: ClientHello<Bytes>,
}

impl AcceptedHello for PassThroughHello {
    fn encoded(&self) -> &HandshakeMessage {
        &self.encoded
    }

    fn client_hello(&self) -> &ClientHello<Bytes> {
        &self.hello
    }
}

impl HelloLayers for PassThroughLayers {
    type Accepted = PassThroughHello;
    type Error = Infallible;
    type Opening<'a> = Ready<Result<PassThroughHello, Infallible>>;

    /// Hands `hello` over; `first` changes nothing.
    fn open<'a>(
        &'a self,
        encoded: HandshakeMessage,
        hello: ClientHello<Bytes>,
        _first: Option<&'a PassThroughHello>,
    ) -> Self::Opening<'a> {
        ready(Ok(PassThroughHello { encoded, hello }))
    }
}

/// Layers whose first ClientHello was opened before the handshake began.
///
/// The first [`HelloLayers::open`] hands that opening over and ignores the
/// ClientHello it is given, which is the one already opened. A second
/// ClientHello, after a HelloRetryRequest, is opened by `L` with the first
/// opening, as the handshake hands it over.
pub struct PreopenedLayers<L: HelloLayers> {
    preopened: Mutex<Option<L::Accepted>>,
    layers: L,
}

impl<L: HelloLayers> PreopenedLayers<L> {
    /// Layers that hand over `preopened` first, and open with `layers`
    /// after.
    pub fn new(preopened: L::Accepted, layers: L) -> Self {
        Self {
            preopened: Mutex::new(Some(preopened)),
            layers,
        }
    }

    /// The opening made before the handshake, until it is handed over.
    fn take(&self) -> Option<L::Accepted> {
        self.preopened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl<L: HelloLayers> HelloLayers for PreopenedLayers<L> {
    type Accepted = L::Accepted;
    type Error = L::Error;
    type Opening<'a>
        = impl Future<Output = Result<L::Accepted, L::Error>> + 'a
    where
        Self: 'a;

    /// Hands the opening made before over for the first ClientHello, and
    /// opens with `L` otherwise. A first ClientHello after the opening was
    /// handed over, as when the layers serve another handshake, is opened
    /// with `L` too.
    fn open<'a>(
        &'a self,
        encoded: HandshakeMessage,
        hello: ClientHello<Bytes>,
        first: Option<&'a L::Accepted>,
    ) -> Self::Opening<'a> {
        let preopened = match first {
            None => self.take(),
            Some(_) => None,
        };
        async move {
            match preopened {
                Some(accepted) => Ok(accepted),
                None => self.layers.open(encoded, hello, first).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AcceptedHello, HelloLayers, PassThroughHello, PassThroughLayers, PreopenedLayers};
    use crate::test_support::{
        VERSIONS, client_hello, extension, ready_now, received_client_hello,
    };

    #[test]
    fn the_hello_passes_through_as_it_came() {
        let bytes = client_hello(&extension(43, VERSIONS));
        let (encoded, hello) = received_client_hello(&bytes);
        let Ok(accepted) = ready_now(PassThroughLayers.open(encoded, hello.clone(), None));
        assert_eq!(accepted.encoded().encoded().as_ref(), &bytes[..]);
        assert_eq!(accepted.client_hello(), &hello);
    }

    /// Layers that record each `first` they are given and pass the hello
    /// through.
    #[derive(Default)]
    struct RecordingLayers {
        firsts: std::sync::Mutex<Vec<Option<Vec<u8>>>>,
    }

    impl HelloLayers for RecordingLayers {
        type Accepted = PassThroughHello;
        type Error = core::convert::Infallible;
        type Opening<'a> = core::future::Ready<Result<PassThroughHello, core::convert::Infallible>>;

        fn open<'a>(
            &'a self,
            encoded: crate::message::HandshakeMessage,
            hello: crate::message::client_hello::ClientHello<bytes::Bytes>,
            first: Option<&'a PassThroughHello>,
        ) -> Self::Opening<'a> {
            self.firsts
                .lock()
                .unwrap()
                .push(first.map(|first| first.encoded().encoded().to_vec()));
            PassThroughLayers.open(encoded, hello, None)
        }
    }

    #[test]
    fn preopened_layers_hand_the_first_opening_over_and_open_the_second() {
        let first_bytes = client_hello(&extension(43, VERSIONS));
        let (encoded, hello) = received_client_hello(&first_bytes);
        let Ok(preopened) = ready_now(PassThroughLayers.open(encoded, hello, None));
        let layers = PreopenedLayers::new(preopened, RecordingLayers::default());

        // The first ClientHello is the one opened before: the wrapped layers
        // are not asked.
        let (encoded, hello) = received_client_hello(&first_bytes);
        let Ok(first) = ready_now(layers.open(encoded, hello, None));
        assert_eq!(first.encoded().encoded().as_ref(), &first_bytes[..]);
        assert!(layers.layers.firsts.lock().unwrap().is_empty());

        // The second goes to the wrapped layers, with the first opening.
        let second_bytes = client_hello(&extension(43, &[6, 0x03, 0x04, 0x03, 0x03, 0x03, 0x02]));
        let (encoded, hello) = received_client_hello(&second_bytes);
        let Ok(second) = ready_now(layers.open(encoded, hello, Some(&first)));
        assert_eq!(second.encoded().encoded().as_ref(), &second_bytes[..]);
        assert_eq!(
            *layers.layers.firsts.lock().unwrap(),
            [Some(first_bytes.clone())]
        );

        // Once handed over, a first ClientHello is opened by the wrapped
        // layers too.
        let (encoded, hello) = received_client_hello(&second_bytes);
        let Ok(again) = ready_now(layers.open(encoded, hello, None));
        assert_eq!(again.encoded().encoded().as_ref(), &second_bytes[..]);
        assert_eq!(layers.layers.firsts.lock().unwrap().len(), 2);
    }
}
