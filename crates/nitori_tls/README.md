# nitori_tls

TLS 1.3 handshake messages, extensions, key schedule and handshake
engines for QUIC.

The baseline is TLS 1.3 ([RFC 9846]) as QUIC version 1 uses it
([RFC 9001]): handshake messages travel in CRYPTO frames, without the
TLS record layer, and the handshake hands traffic secrets to QUIC packet
protection.

It builds without the standard library, with `alloc`. The default `std`
feature adds `clock::SystemClock`, which reads the system's clock, and turns
on the `std` features of `nitori_call`, `nitori_io` and `nitori_sync`.

## Modules

- [`message`]: handshake messages, collected whole from the byte stream
  of an encryption level, with a reader and a writer for each type.
- [`extension`]: the extensions of those messages.
- [`schedule`]: the transcript and the key schedule as one chain of
  values, for the [`role`] of a server and of a client, over a backend
  with the capabilities of [`crypto::TlsCrypto`].
- [`code_point`]: code point lists, such as cipher suites and groups.
- [`cipher_suite`], [`version`], [`alert`]: cipher suites, protocol
  versions and alert descriptions.
- [`vector`]: failures shared by the length-prefixed fields.
- [`clock`]: the current time, as a capability.
- [`key_log`]: the secrets of a handshake, for debugging tools, as a
  capability that logs nothing by default.
- [`verify`]: verification of a server's certificate chain, with
  rustls-webpki and Certificate Transparency.
- [`server`], [`client`]: the capabilities each role's handshake
  consults, such as the choice of parameters and what a ClientHello
  offers, with implementations for plain Web connections, and each
  role's handshake engine, [`server::engine`] and [`client::engine`].
- [`engine`]: what the two engines share: the CRYPTO streams they read
  and write at the current encryption level, and how a handshake fails.

## Messages

The code points are the cipher suites with their hash and AEAD, the
extension types, the handshake types, the alert descriptions and the
protocol versions. The handshake messages, each with its extensions, are
ClientHello, ServerHello and HelloRetryRequest, EncryptedExtensions,
CertificateRequest, Certificate with X.509 certificates or a raw public key,
CertificateVerify, Finished, NewSessionTicket, and the message_hash message of
the transcript.

A whole handshake message is collected from the bytes of an encryption level,
under a limit for each message type; collecting it is a `nitori_call`
operation. The contents of each message, once
[`message::codec::read_handshake_message`] has collected it whole from its
CRYPTO stream, are read and written by synchronous functions in the `codec`
module of their type. The readers run on the bytes of a whole message, a
[`bytes::Buf`] that splits with [`nitori_io::SplitTo`] such as `&[u8]` or
`Bytes`, and their variable-length fields are pieces of those bytes, without
copying; the writers write into a [`bytes::BufMut`] and write a message back
byte for byte. `encoded_len` methods compute lengths without writing. A
recognized extension in a message that may not carry it is rejected, and
unknown extensions are kept. Each read failure answers the TLS alert it calls
for through `alert`.

## Key schedule

The transcript and key schedule form one chain of values that move, with the
role of a server or a client as a type parameter. Each step consumes the one
before: the Handshake traffic secrets need the ServerHello, the 1-RTT secrets
need the server's Finished, and a client reads the server's Finished only after
the server has authenticated with a certificate or with the PSK it accepted.
The chain derives, over a backend with the capabilities of `nitori_crypto`:

- the traffic secrets for QUIC packet protection, each with the hash of its
  suite;
- the Finished messages and PSK binders;
- the 0-RTT secrets;
- the exporter and the PSKs of tickets.

## Handshake engines

The handshake engines of a server and of a client run in steps. A trait for
each role, which the engine's start state implements, gives the steps: each
step takes the state before it by value and returns the traffic secrets or the
completion of the handshake, with its exporter, together with the state of the
next step. Each step is also a `nitori_call` operation for every implementation
of the trait.

The states own the backend and the role's capabilities. A step reads and writes
the CRYPTO bytes of the current encryption level through one access the
connection lends on every poll, and asks the connection for transport
parameters through synchronous methods. A server holds back the client's 1-RTT
secret until it has verified the client's Finished, and admits 0-RTT only when
the handshake, the connection and its ticket policy all agree.

The engines do not yet open encrypted ClientHello layers or authenticate
clients with certificates.

## Shared state

The ready-made capabilities that connections share through `&self`, such as
the ticket stores of both roles, the memory of retried groups, the trust
anchors, the distrusted keys, the CT log list and the manual clock, name no
lock: each keeps its state in the wrapper of a `nitori_sync::Synchronization`
family that the host chooses, `RefCellSynchronization` on one thread or, with
the `std` feature, `MutexSynchronization`. Those that keep a hash table also
take its hasher as a type parameter, `S: BuildHasher + Default`; none of their
keys is chosen by the peer.

[RFC 9846]: https://www.rfc-editor.org/rfc/rfc9846.html
[RFC 9001]: https://www.rfc-editor.org/rfc/rfc9001.html
