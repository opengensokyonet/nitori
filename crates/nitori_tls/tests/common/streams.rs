//! CRYPTO streams and transport parameters in memory, as the receiver a
//! handshake engine's steps are lent: a buffer per level for what a side
//! receives and for what it sends.

use bytes::{Buf, BufMut, Bytes};
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use nitori_call::{BorrowedReceiver, family_receiver};
use nitori_io::{Read, Write};
use nitori_tls::{
    client::engine::ClientTransport,
    engine::{CryptoInput, CryptoLevel, CryptoOutput},
    extension::OpaqueData,
    server::{
        engine::{Negotiated, ServerTransport},
        targets::SelectedTarget,
        ticket::TicketState,
    },
};
use std::collections::VecDeque;

/// A failure of the receivers in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Bytes of the level left were not read.
    Unread(CryptoLevel),
    /// The transport parameters were refused.
    Parameters,
}

pub fn index(level: CryptoLevel) -> usize {
    match level {
        CryptoLevel::Initial => 0,
        CryptoLevel::Handshake => 1,
        CryptoLevel::OneRtt => 2,
    }
}

pub const LEVELS: [CryptoLevel; 3] = [
    CryptoLevel::Initial,
    CryptoLevel::Handshake,
    CryptoLevel::OneRtt,
];

/// What one side receives: a buffer per level and the current level.
pub struct Incoming {
    pub levels: [VecDeque<u8>; 3],
    pub current: CryptoLevel,
}

impl Default for Incoming {
    fn default() -> Self {
        Self {
            levels: Default::default(),
            current: CryptoLevel::Initial,
        }
    }
}

impl Incoming {
    /// Moves what is buffered at the current level into `destination`;
    /// waits while nothing is.
    fn poll_read<Output: BufMut + ?Sized>(&mut self, destination: &mut Output) -> Poll<usize> {
        let buffer = &mut self.levels[index(self.current)];
        if buffer.is_empty() {
            return Poll::Pending;
        }
        let len = buffer.len().min(destination.remaining_mut());
        for byte in buffer.drain(..len) {
            destination.put_u8(byte);
        }
        Poll::Ready(len)
    }

    /// Moves to `level`, unless bytes of the current level are unread.
    fn move_to(&mut self, level: CryptoLevel) -> Result<(), Fault> {
        if !self.levels[index(self.current)].is_empty() {
            return Err(Fault::Unread(self.current));
        }
        self.current = level;
        Ok(())
    }
}

/// What one side sends: a buffer per level and the current level.
pub struct Outgoing {
    pub levels: [Vec<u8>; 3],
    pub current: CryptoLevel,
}

impl Default for Outgoing {
    fn default() -> Self {
        Self {
            levels: Default::default(),
            current: CryptoLevel::Initial,
        }
    }
}

impl Outgoing {
    /// Appends `input` to the current level.
    fn write(&mut self, input: &mut (impl Buf + ?Sized)) -> usize {
        let buffer = &mut self.levels[index(self.current)];
        let len = input.remaining();
        while input.has_remaining() {
            let chunk = input.chunk();
            let chunk_len = chunk.len();
            buffer.extend_from_slice(chunk);
            input.advance(chunk_len);
        }
        len
    }
}

/// The server's connection side of the transport parameters.
pub struct ServerParameters {
    pub local: Bytes,
    pub peer: Option<Bytes>,
    pub early_data: bool,
    pub refuse: bool,
}

impl ServerParameters {
    fn negotiate(
        &mut self,
        peer: &[u8],
        resumed: Option<&TicketState<&'static str>>,
    ) -> Result<Negotiated, Fault> {
        if self.refuse {
            return Err(Fault::Parameters);
        }
        self.peer = Some(Bytes::copy_from_slice(peer));
        let compatible =
            resumed.is_some_and(|state| state.resumable.transport_parameters == self.local);
        Ok(Negotiated {
            local: OpaqueData::new(self.local.clone()).unwrap(),
            early_data: self.early_data && compatible,
        })
    }
}

/// The client's connection side of the transport parameters.
pub struct ClientParameters {
    pub local: Bytes,
    pub peer: Option<Bytes>,
}

/// What one side's handshake is lent: its CRYPTO streams and its side of
/// the transport parameters, as one receiver.
pub struct Side<P> {
    pub input: Incoming,
    pub output: Outgoing,
    pub parameters: P,
}

/// The server's receiver.
pub type ServerSide = Side<ServerParameters>;

/// The client's receiver.
pub type ClientSide = Side<ClientParameters>;

family_receiver!(impl [P: Unpin + 'static] for Side<P>);

impl<P: Unpin + 'static> Read for Side<P> {
    type Error = Fault;

    fn poll_read<'v, Output: BufMut + ?Sized>(
        host: Pin<&mut BorrowedReceiver<'v, Side<P>>>,
        _: &mut Context<'_>,
        destination: &mut Output,
    ) -> Poll<Result<usize, Fault>>
    where
        Self: 'v,
    {
        host.get_pin_mut()
            .get_mut()
            .input
            .poll_read(destination)
            .map(Ok)
    }
}

impl<P: Unpin + 'static> CryptoInput for Side<P> {
    type LevelError = Fault;

    fn move_input_to<'v>(
        view: Pin<&mut BorrowedReceiver<'v, Side<P>>>,
        level: CryptoLevel,
    ) -> Result<(), Fault>
    where
        Self: 'v,
    {
        view.get_pin_mut().get_mut().input.move_to(level)
    }
}

impl<P: Unpin + 'static> Write for Side<P> {
    type Error = Fault;

    fn poll_write<'v, Input: Buf + ?Sized>(
        host: Pin<&mut BorrowedReceiver<'v, Side<P>>>,
        _: &mut Context<'_>,
        input: &mut Input,
    ) -> Poll<Result<usize, Fault>>
    where
        Self: 'v,
    {
        Poll::Ready(Ok(host.get_pin_mut().get_mut().output.write(input)))
    }
}

impl<P: Unpin + 'static> CryptoOutput for Side<P> {
    fn move_output_to<'v>(view: Pin<&mut BorrowedReceiver<'v, Side<P>>>, level: CryptoLevel)
    where
        Self: 'v,
    {
        view.get_pin_mut().get_mut().output.current = level;
    }
}

impl ServerTransport<&'static str> for ServerSide {
    type Error = Fault;

    fn negotiate<'v>(
        view: Pin<&mut BorrowedReceiver<'v, ServerSide>>,
        peer: &[u8],
        _selected: &SelectedTarget<&'static str>,
        resumed: Option<&TicketState<&'static str>>,
    ) -> Result<Negotiated, Fault>
    where
        Self: 'v,
    {
        view.get_pin_mut()
            .get_mut()
            .parameters
            .negotiate(peer, resumed)
    }
}

impl ClientTransport for ClientSide {
    type Error = Fault;

    fn local<'v>(view: Pin<&mut BorrowedReceiver<'v, ClientSide>>) -> OpaqueData<Bytes>
    where
        Self: 'v,
    {
        OpaqueData::new(view.get_pin_mut().get_mut().parameters.local.clone()).unwrap()
    }

    fn peer<'v>(view: Pin<&mut BorrowedReceiver<'v, ClientSide>>, peer: &[u8]) -> Result<(), Fault>
    where
        Self: 'v,
    {
        view.get_pin_mut().get_mut().parameters.peer = Some(Bytes::copy_from_slice(peer));
        Ok(())
    }
}
