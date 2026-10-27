//! The frame format as tokio-util codecs: the sans-IO way to read and write
//! frames. Decoding hands out a field section once all of it is buffered, and
//! DATA in whatever pieces are buffered, without copying.
use crate::{
    common::Counters,
    fields::Fields,
    workload::{Frame, FrameHeader, Kind, Protocol, data_checksum},
};
use bytes::{Buf, BytesMut};
use std::{io, marker::PhantomData};
use tokio_util::codec::{Decoder, Encoder};

pub enum Item {
    /// A whole field section.
    Headers(BytesMut),
    /// A piece of DATA payload.
    Data(BytesMut),
}

pub struct FrameDecoder<P> {
    /// The header of a HEADERS frame whose field section is incomplete.
    pending: Option<FrameHeader>,
    /// DATA payload still to come.
    remaining: usize,
    protocol: PhantomData<P>,
}
impl<P> Default for FrameDecoder<P> {
    fn default() -> Self {
        Self {
            pending: None,
            remaining: 0,
            protocol: PhantomData,
        }
    }
}
impl<P: Protocol> Decoder for FrameDecoder<P> {
    type Item = Item;
    type Error = io::Error;
    fn decode(&mut self, source: &mut BytesMut) -> io::Result<Option<Item>> {
        if self.remaining == 0 && self.pending.is_none() {
            let mut length = 0;
            loop {
                let needed = P::header_needed(&source[..length]);
                if needed == 0 {
                    break;
                }
                if length + needed > source.len() {
                    return Ok(None);
                }
                length += needed;
            }
            let header = P::parse_header(&source[..length]);
            source.advance(length);
            match header.kind {
                Kind::Headers => self.pending = Some(header),
                Kind::Data if header.length == 0 => return Ok(Some(Item::Data(BytesMut::new()))),
                Kind::Data => self.remaining = header.length,
            }
        }
        if let Some(header) = self.pending {
            if source.len() < header.length {
                source.reserve(header.length - source.len());
                return Ok(None);
            }
            self.pending = None;
            return Ok(Some(Item::Headers(source.split_to(header.length))));
        }
        if source.is_empty() {
            return Ok(None);
        }
        let count = self.remaining.min(source.len());
        self.remaining -= count;
        Ok(Some(Item::Data(source.split_to(count))))
    }
}

/// Process a decoded item: decode a field section, or visit DATA.
pub fn process<P: Protocol>(item: Item, fields: &mut Fields) -> u64 {
    match item {
        Item::Headers(block) => {
            P::decode_fields(&block, fields).expect("a valid field section");
            fields.checksum()
        }
        Item::Data(payload) => data_checksum(&payload),
    }
}

/// Process items while keeping track of the codec's buffer.
pub struct Processor<P> {
    pub fields: Fields,
    pub checksum: u64,
    protocol: PhantomData<P>,
}
impl<P: Protocol> Default for Processor<P> {
    fn default() -> Self {
        Self {
            fields: Fields::default(),
            checksum: 0,
            protocol: PhantomData,
        }
    }
}
impl<P: Protocol> Processor<P> {
    pub fn add(&mut self, item: Item) {
        self.checksum += process::<P>(item, &mut self.fields);
    }
    pub fn note_buffer(&self, counters: &Counters, buffer: &BytesMut) {
        counters.buffer_at_least(buffer.capacity());
    }
}

/// Encodes frames, field sections included, into the codec's buffer.
pub struct FrameEncoder<P> {
    fields: Vec<u8>,
    header: Vec<u8>,
    protocol: PhantomData<P>,
}
impl<P> Default for FrameEncoder<P> {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            header: Vec::new(),
            protocol: PhantomData,
        }
    }
}
impl<P: Protocol> Encoder<Frame<'_>> for FrameEncoder<P> {
    type Error = io::Error;
    fn encode(&mut self, frame: Frame<'_>, output: &mut BytesMut) -> io::Result<()> {
        let (header, payload) = frame.parts::<P>(&mut self.fields, &mut self.header);
        output.reserve(header.len() + payload.len());
        output.extend_from_slice(header);
        output.extend_from_slice(payload);
        Ok(())
    }
}
