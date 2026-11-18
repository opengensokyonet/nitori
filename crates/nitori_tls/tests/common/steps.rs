//! The steps of each role's handshake run one after another as one call,
//! as a connection runs them, yielding what each step returns.

use core::convert::Infallible;
use nitori_call::{LentAccess, call};
use nitori_tls::{
    client::engine::{
        ClientHandshakeCapability, ClientHandshakeCompleted, ClientHandshakeSteps, ClientTransport,
        EarlyDataOffered, ServerHelloRead,
    },
    crypto::TlsCrypto,
    engine::HandshakeError,
    message::MessageLimits,
    schedule::OneRttSendTrafficSecret,
    server::engine::{
        FirstClientHello, ServerHandshakeCapability, ServerHandshakeCompleted,
        ServerHandshakeSteps, ServerHelloWritten, ServerTransport, read_first_client_hello,
    },
};

/// What a server's steps return, in this order.
pub enum ServerEvent<C: TlsCrypto, T> {
    /// [`ServerHandshakeSteps::write_server_hello`].
    ServerHelloWritten(ServerHelloWritten<T>),
    /// [`ServerHandshakeSteps::write_server_finished`].
    ServerFinishedWritten(OneRttSendTrafficSecret),
    /// [`ServerHandshakeSteps::read_client_finished`].
    Completed(ServerHandshakeCompleted<C, T>),
}

/// What a client's steps return, in this order.
pub enum ClientEvent<C: TlsCrypto> {
    /// [`ClientHandshakeSteps::write_client_hello`], when it offers 0-RTT.
    EarlyDataOffered(EarlyDataOffered),
    /// [`ClientHandshakeSteps::read_server_hello`].
    ServerHelloRead(ServerHelloRead),
    /// [`ClientHandshakeSteps::read_server_finished`].
    Completed(ClientHandshakeCompleted<C>),
}

/// The failures of a server's steps.
pub type ServerFailed<E, W> = HandshakeError<
    <E as ServerHandshakeSteps>::AlertError,
    <W as ServerTransport<<E as ServerHandshakeSteps>::Target>>::Error,
>;

/// The failures of a client's steps.
pub type ClientFailed<E, W> =
    HandshakeError<<E as ClientHandshakeSteps>::AlertError, <W as ClientTransport>::Error>;

/// Reads the first ClientHello within `limits` and runs the steps of the
/// server's handshake `tls`.
#[call(yields = ServerEvent<<E as ServerHandshakeSteps>::Crypto, <E as ServerHandshakeSteps>::Target>)]
pub async fn server_steps<W, E>(
    host: &mut impl LentAccess<Family = W>,
    tls: E,
    limits: MessageLimits,
) -> Result<(), ServerFailed<E, W>>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    let first = read_first_client_hello(host, limits)
        .await
        .map_err(|error| error.map(Into::into, W::convert_read_error))?;
    let (written, state) = tls.write_server_hello(first, host).await?;
    yield ServerEvent::ServerHelloWritten(written);
    let (send, state) = E::write_server_finished(state, host).await?;
    yield ServerEvent::ServerFinishedWritten(send);
    let (completed, state) = E::read_client_finished(state, host).await?;
    yield ServerEvent::Completed(completed);
    E::write_tickets(state, host).await
}

/// Runs the steps of the server's handshake `tls` from `first`, a first
/// ClientHello read before the engine was built.
#[call(yields = ServerEvent<<E as ServerHandshakeSteps>::Crypto, <E as ServerHandshakeSteps>::Target>)]
pub async fn server_steps_from<W, E>(
    host: &mut impl LentAccess<Family = W>,
    tls: E,
    first: FirstClientHello,
) -> Result<(), ServerFailed<E, W>>
where
    E: ServerHandshakeSteps,
    W: ServerHandshakeCapability<<E as ServerHandshakeSteps>::Target>,
{
    let (written, state) = tls.write_server_hello(first, host).await?;
    yield ServerEvent::ServerHelloWritten(written);
    let (send, state) = E::write_server_finished(state, host).await?;
    yield ServerEvent::ServerFinishedWritten(send);
    let (completed, state) = E::read_client_finished(state, host).await?;
    yield ServerEvent::Completed(completed);
    E::write_tickets(state, host).await
}

/// Runs the steps of the client's handshake `tls`; it ends only when it
/// fails.
#[call(yields = ClientEvent<<E as ClientHandshakeSteps>::Crypto>)]
pub async fn client_steps<W, E>(
    host: &mut impl LentAccess<Family = W>,
    tls: E,
) -> Result<Infallible, ClientFailed<E, W>>
where
    E: ClientHandshakeSteps,
    W: ClientHandshakeCapability,
{
    let (offered, state) = tls.write_client_hello(host).await?;
    if let Some(offered) = offered {
        yield ClientEvent::EarlyDataOffered(offered);
    }
    let (read, state) = E::read_server_hello(state, host).await?;
    yield ClientEvent::ServerHelloRead(read);
    let (completed, state) = E::read_server_finished(state, host).await?;
    yield ClientEvent::Completed(completed);
    E::read_tickets(state, host).await
}
