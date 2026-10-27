//! HTTP responses from a captured corpus, framed as HTTP/2 (RFC 9113) with
//! HPACK or as HTTP/3 (RFC 9114) with QPACK.
//!
//! The header lists are the 383 responses of `data/fb-resp.qif` from the QPACK
//! interop corpus (<https://github.com/qpackers/qifs>, MIT, see
//! `data/LICENSE-qifs.md`). Names are lowercased and `status` becomes the
//! `:status` pseudo-header, placed first. A response's body is as long as its
//! `content-length`; responses without one take a length drawn from those
//! that have one. Bodies are split into DATA frames of at most the chunk size.
use crate::{
    common::byte_sum,
    fields::{FieldError, Fields, fields_checksum, hpack, qpack},
};
use bytes::BufMut;
use std::sync::Arc;

pub struct Message {
    pub fields: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// `passes` copies of the corpus, each body filled with a distinct pattern.
pub fn messages(passes: usize) -> Arc<Vec<Message>> {
    let corpus = include_str!("../data/fb-resp.qif");
    let mut lists: Vec<Vec<(String, String)>> = Vec::new();
    for section in corpus.split("\n\n").filter(|s| !s.trim().is_empty()) {
        let mut fields: Vec<(String, String)> = section
            .lines()
            .map(|line| {
                let (name, value) = line.split_once('\t').expect("a QIF field line");
                let name = name.to_ascii_lowercase();
                let name = if name == "status" {
                    ":status".into()
                } else {
                    name
                };
                (name, value.to_owned())
            })
            .collect();
        fields.sort_by_key(|(name, _)| !name.starts_with(':'));
        lists.push(fields);
    }
    let lengths: Vec<usize> = lists
        .iter()
        .filter_map(|fields| content_length(fields))
        .collect();
    let mut state = 0x9e37_79b9_u32;
    let mut messages = Vec::new();
    for pass in 0..passes {
        for (index, fields) in lists.iter().enumerate() {
            let length = content_length(fields).unwrap_or_else(|| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                lengths[state as usize % lengths.len()]
            });
            let salt = (pass * lists.len() + index) as u8;
            let body = (0..length)
                .map(|offset| (offset as u8).wrapping_mul(31).wrapping_add(salt))
                .collect();
            messages.push(Message {
                fields: fields.clone(),
                body,
            });
        }
    }
    Arc::new(messages)
}

fn content_length(fields: &[(String, String)]) -> Option<usize> {
    fields
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
}

impl Message {
    fn field_strs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
    /// What a reader computes from this message's frames.
    pub fn checksum(&self) -> u64 {
        fields_checksum(
            self.fields
                .iter()
                .map(|(n, v)| (n.as_bytes(), v.as_bytes())),
        ) + data_checksum(&self.body)
    }
}

/// The checksum of a piece of DATA payload; it adds up over pieces.
pub fn data_checksum(bytes: &[u8]) -> u64 {
    byte_sum(bytes) + 3 * bytes.len() as u64
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Headers,
    Data,
}

#[derive(Clone, Copy, Debug)]
pub struct FrameHeader {
    pub kind: Kind,
    pub length: usize,
}

/// A frame format and its field compression.
pub trait Protocol: Copy + Default + Send + Sync + 'static {
    /// How many more bytes the frame header beginning with `prefix` needs at
    /// least; 0 once it is complete.
    fn header_needed(prefix: &[u8]) -> usize;
    /// Parse a complete frame header.
    fn parse_header(header: &[u8]) -> FrameHeader;
    fn write_header(header: FrameHeader, stream: u32, end_stream: bool, output: &mut impl BufMut);
    fn encode_fields<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>, output: &mut Vec<u8>);
    fn decode_fields(input: &[u8], fields: &mut Fields) -> Result<(), FieldError>;
}

/// HTTP/2 frames: a 9-byte header of a 24-bit length, a type, flags and a
/// stream identifier. Field blocks use HPACK.
#[derive(Clone, Copy, Default)]
pub struct Http2;
impl Protocol for Http2 {
    fn header_needed(prefix: &[u8]) -> usize {
        9 - prefix.len()
    }
    fn parse_header(header: &[u8]) -> FrameHeader {
        let length = u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize;
        let kind = match header[3] {
            0x0 => Kind::Data,
            0x1 => Kind::Headers,
            other => panic!("unexpected HTTP/2 frame type {other}"),
        };
        FrameHeader { kind, length }
    }
    fn write_header(header: FrameHeader, stream: u32, end_stream: bool, output: &mut impl BufMut) {
        output.put_slice(&(header.length as u32).to_be_bytes()[1..]);
        let (kind, flags) = match header.kind {
            Kind::Data => (0x0, 0),
            // END_HEADERS: field blocks never need CONTINUATION here.
            Kind::Headers => (0x1, 0x4),
        };
        output.put_u8(kind);
        output.put_u8(flags | if end_stream { 0x1 } else { 0 });
        output.put_u32(stream);
    }
    fn encode_fields<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>, output: &mut Vec<u8>) {
        hpack::encode(fields, output)
    }
    fn decode_fields(input: &[u8], fields: &mut Fields) -> Result<(), FieldError> {
        hpack::decode(input, fields)
    }
}

/// HTTP/3 frames: a variable-length integer type and length. Field sections
/// use QPACK.
#[derive(Clone, Copy, Default)]
pub struct Http3;
fn varint_length(first: u8) -> usize {
    1 << (first >> 6)
}
fn read_varint(bytes: &[u8]) -> u64 {
    let mut value = (bytes[0] & 0x3f) as u64;
    for &byte in &bytes[1..varint_length(bytes[0])] {
        value = (value << 8) | byte as u64;
    }
    value
}
fn write_varint(value: u64, output: &mut impl BufMut) {
    match value {
        0..=0x3f => output.put_u8(value as u8),
        0x40..=0x3fff => output.put_u16(value as u16 | 0x4000),
        0x4000..=0x3fff_ffff => output.put_u32(value as u32 | 0x8000_0000),
        _ => output.put_u64(value | 0xc000_0000_0000_0000),
    }
}
impl Protocol for Http3 {
    fn header_needed(prefix: &[u8]) -> usize {
        let Some(&first) = prefix.first() else {
            return 1;
        };
        let type_length = varint_length(first);
        if prefix.len() <= type_length {
            return type_length + 1 - prefix.len();
        }
        type_length + varint_length(prefix[type_length]) - prefix.len()
    }
    fn parse_header(header: &[u8]) -> FrameHeader {
        let kind = match read_varint(header) {
            0x0 => Kind::Data,
            0x1 => Kind::Headers,
            other => panic!("unexpected HTTP/3 frame type {other}"),
        };
        let length = read_varint(&header[varint_length(header[0])..]) as usize;
        FrameHeader { kind, length }
    }
    fn write_header(header: FrameHeader, _: u32, _: bool, output: &mut impl BufMut) {
        write_varint(
            match header.kind {
                Kind::Data => 0x0,
                Kind::Headers => 0x1,
            },
            output,
        );
        write_varint(header.length as u64, output);
    }
    fn encode_fields<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>, output: &mut Vec<u8>) {
        qpack::encode(fields, output)
    }
    fn decode_fields(input: &[u8], fields: &mut Fields) -> Result<(), FieldError> {
        qpack::decode(input, fields)
    }
}

/// One frame to send: the HEADERS of a message, or a piece of its body.
#[derive(Clone, Copy)]
pub enum Frame<'m> {
    Headers {
        stream: u32,
        message: &'m Message,
    },
    Data {
        stream: u32,
        payload: &'m [u8],
        end_stream: bool,
    },
}

impl<'m> Frame<'m> {
    /// The frame header and payload, encoding the field section into
    /// `fields` and the header into `header`. The payload of DATA is the
    /// message body itself.
    pub fn parts<'s, P: Protocol>(
        &self,
        fields: &'s mut Vec<u8>,
        header: &'s mut Vec<u8>,
    ) -> (&'s [u8], &'s [u8])
    where
        'm: 's,
    {
        header.clear();
        match *self {
            Frame::Headers { stream, message } => {
                fields.clear();
                P::encode_fields(message.field_strs(), fields);
                let frame = FrameHeader {
                    kind: Kind::Headers,
                    length: fields.len(),
                };
                P::write_header(frame, stream, message.body.is_empty(), header);
                (header, fields)
            }
            Frame::Data {
                stream,
                payload,
                end_stream,
            } => {
                let frame = FrameHeader {
                    kind: Kind::Data,
                    length: payload.len(),
                };
                P::write_header(frame, stream, end_stream, header);
                (header, payload)
            }
        }
    }

    /// Append the whole frame.
    pub fn encode<P: Protocol>(
        &self,
        fields: &mut Vec<u8>,
        header: &mut Vec<u8>,
        output: &mut impl BufMut,
    ) {
        let (header, payload) = self.parts::<P>(fields, header);
        output.put_slice(header);
        output.put_slice(payload);
    }
}

/// The frames of one message.
pub fn message_frames(
    stream: u32,
    message: &Message,
    chunk: usize,
) -> impl Iterator<Item = Frame<'_>> {
    let pieces = message.body.len().div_ceil(chunk);
    std::iter::once(Frame::Headers { stream, message }).chain((0..pieces).map(move |piece| {
        let start = piece * chunk;
        let end = (start + chunk).min(message.body.len());
        Frame::Data {
            stream,
            payload: &message.body[start..end],
            end_stream: end == message.body.len(),
        }
    }))
}

/// The frames of an HTTP/2 connection with `streams` responses in flight at a
/// time, interleaved frame by frame. Each response takes the next odd stream
/// identifier.
pub fn connection_frames(
    messages: &[Message],
    streams: usize,
    chunk: usize,
) -> impl Iterator<Item = Frame<'_>> {
    let mut next_message = 0;
    let mut next_stream = 1u32;
    let mut active: Vec<Box<dyn Iterator<Item = Frame<'_>> + '_>> = Vec::new();
    let mut turn = 0;
    std::iter::from_fn(move || {
        loop {
            while active.len() < streams && next_message < messages.len() {
                active.push(Box::new(message_frames(
                    next_stream,
                    &messages[next_message],
                    chunk,
                )));
                next_message += 1;
                next_stream += 2;
            }
            if active.is_empty() {
                return None;
            }
            turn %= active.len();
            match active[turn].next() {
                Some(frame) => {
                    turn += 1;
                    return Some(frame);
                }
                None => {
                    drop(active.remove(turn));
                }
            }
        }
    })
}

/// The frames of the messages carried on stream slot `slot` of `streams`:
/// every `streams`-th message, one after another.
pub fn slot_frames(
    messages: &[Message],
    slot: usize,
    streams: usize,
    chunk: usize,
) -> impl Iterator<Item = Frame<'_>> {
    messages
        .iter()
        .skip(slot)
        .step_by(streams)
        .flat_map(move |message| message_frames(0, message, chunk))
}

/// Encode frames into bytes.
pub fn encode_all<'m, P: Protocol>(frames: impl Iterator<Item = Frame<'m>>) -> Vec<u8> {
    let (mut fields, mut header) = (Vec::new(), Vec::new());
    let mut output = Vec::new();
    for frame in frames {
        frame.encode::<P>(&mut fields, &mut header, &mut output);
    }
    output
}

pub fn total_checksum<'m>(messages: impl Iterator<Item = &'m Message>) -> u64 {
    messages.map(Message::checksum).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_parses_and_sizes_bodies() {
        let messages = messages(1);
        assert_eq!(messages.len(), 383);
        assert!(messages.iter().all(|m| m.fields[0].0 == ":status"));
        let with_length = messages
            .iter()
            .filter(|m| content_length(&m.fields) == Some(m.body.len()))
            .count();
        assert_eq!(with_length, 287);
    }

    #[test]
    fn http3_headers_round_trip() {
        for (kind, length) in [(Kind::Headers, 5), (Kind::Data, 70), (Kind::Data, 20_000)] {
            let mut bytes = Vec::new();
            Http3::write_header(FrameHeader { kind, length }, 0, false, &mut bytes);
            for split in 0..bytes.len() {
                assert!(Http3::header_needed(&bytes[..split]) > 0);
            }
            assert_eq!(Http3::header_needed(&bytes), 0);
            let parsed = Http3::parse_header(&bytes);
            assert_eq!((parsed.kind, parsed.length), (kind, length));
        }
    }

    #[test]
    fn interleaving_keeps_every_frame() {
        let messages = messages(1);
        let all: usize = messages
            .iter()
            .map(|m| 1 + m.body.len().div_ceil(4096))
            .sum();
        assert_eq!(connection_frames(&messages, 8, 4096).count(), all);
        let slots: usize = (0..8)
            .map(|s| slot_frames(&messages, s, 8, 4096).count())
            .sum();
        assert_eq!(slots, all);
    }
}
