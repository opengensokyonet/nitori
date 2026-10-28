//! The QPACK encoder stream (RFC 9204 Section 4.3): the instructions that
//! build a decoder's dynamic table, received or sent over the stream buffers
//! of [`h3`](crate::h3), one encoder stream per connection and one task per
//! stream.
//!
//! Unlike frames, instructions carry no length. Where one ends is known only
//! by parsing its prefix integers, and most are a few bytes long; their
//! strings may be Huffman-coded. The encoder ([`EncoderTable`]) inserts every
//! field of the corpus that is not in the static table, referring to a name
//! in either table where it can, and duplicates an entry it uses again once
//! the entry is among the oldest quarter of the table. Field sections and the
//! decoder stream are left out; nothing keeps entries from eviction.
//!
//! Receiving: ordinary async and calls read an instruction a byte at a time
//! until its strings, which they decode piece by piece as the pieces arrive,
//! without collecting the instruction first. The codec finds where the
//! complete instructions end under the lock and decodes them after unlocking.
//!
//! Sending: the writer goes through the field sections of its connection and
//! writes the instructions each needs before moving on to the next, as an
//! encoder must before the section that refers to them is sent; choosing and
//! encoding the instructions is the work. Ordinary async and calls write an
//! instruction as its prefix integers and strings, one after another, each
//! integer from the stack and a Huffman-coded string a piece at a time
//! through a small buffer on the stack. The codec encodes whole instructions
//! into the buffer of `FramedWrite`, flushed after each field section. The
//! connection task decodes and applies what it takes, after unlocking.
use crate::{
    calls::{Consume, ConsumeExt as _, Release, Shared, Take, TakeExt as _, Write, write_all},
    common::{
        ByteSource, Counters, Lock, LockKind, LockPerLeaf, Pace, PerLock, PollConsume, PollLock,
        PollWrite, Sample, byte_sum, counted, plain_write_all, with_lock, woken,
    },
    fields::{
        FieldError, Huffman, HuffmanEncoder, decode_integer, decode_string, encode_integer,
        encode_string, huffman_length, qpack,
    },
    h3::{
        Check, RecvStream, SendStream, StreamWriter, close, deliver, drain, per_stream, recv_conn,
        send_conn,
    },
    workload::Message,
};
use bytes::BytesMut;
use futures_sink::Sink;
use nitori_call::{ReceiverAccess, call};
use std::{
    collections::{HashMap, VecDeque},
    future::poll_fn,
    io,
    pin::Pin,
    sync::Arc,
    task::{Poll, ready},
    time::Instant,
};
use tokio::runtime::Runtime;
use tokio_util::codec::{Decoder, Encoder, FramedWrite};

fn invalid(_: FieldError) -> io::Error {
    io::ErrorKind::InvalidData.into()
}

/// The checksum of an entry, as for a decoded field.
fn entry_checksum(name: &[u8], value: &[u8]) -> u64 {
    byte_sum(name) + byte_sum(value) + 7
}

fn entry_size(name: &[u8], value: &[u8]) -> usize {
    name.len() + value.len() + 32
}

// The encoder.

/// An encoder stream instruction, borrowing its strings from a field.
#[derive(Clone, Copy)]
pub enum Instruction<'f> {
    /// Set Dynamic Table Capacity.
    Capacity(usize),
    /// Duplicate the entry at a relative index.
    Duplicate(usize),
    /// Insert with Name Reference, to the static table or to a relative index
    /// in the dynamic table.
    NameReference {
        is_static: bool,
        index: usize,
        value: &'f str,
    },
    /// Insert with Literal Name.
    LiteralName { name: &'f str, value: &'f str },
}
impl Instruction<'_> {
    pub fn encode(&self, output: &mut Vec<u8>) {
        match *self {
            Instruction::Capacity(capacity) => encode_integer(capacity, 5, 0x20, output),
            Instruction::Duplicate(index) => encode_integer(index, 5, 0x00, output),
            Instruction::NameReference {
                is_static,
                index,
                value,
            } => {
                let flags = if is_static { 0xc0 } else { 0x80 };
                encode_integer(index, 6, flags, output);
                encode_string(value.as_bytes(), 7, 0x00, output);
            }
            Instruction::LiteralName { name, value } => {
                encode_string(name.as_bytes(), 5, 0x40, output);
                encode_string(value.as_bytes(), 7, 0x00, output);
            }
        }
    }
}

/// The newest absolute index of a name, and of each of its values.
#[derive(Default)]
struct Name {
    newest: usize,
    values: HashMap<String, usize>,
}

/// The encoder's copy of the dynamic table. It inserts every field that is
/// not in the static table, referring to a name in either table where it can,
/// and duplicates an entry it uses again once the entry is among the oldest
/// quarter of the table.
pub struct EncoderTable {
    capacity: usize,
    size: usize,
    /// The size of each entry, oldest first.
    entries: VecDeque<usize>,
    /// The absolute index of the oldest entry.
    dropped: usize,
    names: HashMap<String, Name>,
}
impl EncoderTable {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            size: 0,
            entries: VecDeque::new(),
            dropped: 0,
            names: HashMap::new(),
        }
    }
    /// The instruction that sets the table's capacity, which comes first.
    pub fn start(&self) -> Instruction<'static> {
        Instruction::Capacity(self.capacity)
    }
    fn inserted(&self) -> usize {
        self.dropped + self.entries.len()
    }
    fn live(&self, index: usize) -> bool {
        index >= self.dropped
    }
    /// Whether an entry is among the oldest quarter, soon to be evicted.
    fn draining(&self, index: usize) -> bool {
        index - self.dropped < self.entries.len() / 4
    }
    /// The relative index of an entry for the next instruction.
    fn relative(&self, index: usize) -> usize {
        self.inserted() - 1 - index
    }
    /// The instruction that a field needs, if any. The table changes as the
    /// decoder's will once it applies the instruction.
    pub fn instruction<'f>(&mut self, name: &'f str, value: &'f str) -> Option<Instruction<'f>> {
        let size = entry_size(name.as_bytes(), value.as_bytes());
        if qpack::static_field(name, value).is_some() || size > self.capacity {
            return None;
        }
        let known = self.names.get(name);
        let instruction = if let Some(&index) = known.and_then(|known| known.values.get(value))
            && self.live(index)
        {
            if !self.draining(index) {
                return None;
            }
            Instruction::Duplicate(self.relative(index))
        } else if let Some(index) = qpack::static_name(name) {
            Instruction::NameReference {
                is_static: true,
                index,
                value,
            }
        } else if let Some(known) = known
            && self.live(known.newest)
        {
            Instruction::NameReference {
                is_static: false,
                index: self.relative(known.newest),
                value,
            }
        } else {
            Instruction::LiteralName { name, value }
        };
        self.insert(name, value, size);
        Some(instruction)
    }
    fn insert(&mut self, name: &str, value: &str, size: usize) {
        while self.size + size > self.capacity {
            self.size -= self.entries.pop_front().unwrap();
            self.dropped += 1;
        }
        let index = self.inserted();
        let known = match self.names.get_mut(name) {
            Some(known) => known,
            None => self.names.entry(name.into()).or_default(),
        };
        known.newest = index;
        match known.values.get_mut(value) {
            Some(newest) => *newest = index,
            None => {
                known.values.insert(value.into(), index);
            }
        }
        self.entries.push_back(size);
        self.size += size;
    }
}

/// One connection's encoder stream.
pub struct Instructions {
    pub bytes: Vec<u8>,
    pub count: usize,
    /// The sum of the checksums of every entry inserted.
    pub checksum: u64,
}

/// The encoder stream for `messages` with a table of `capacity` bytes.
pub fn encode<'m>(messages: impl Iterator<Item = &'m Message>, capacity: usize) -> Instructions {
    let mut table = EncoderTable::new(capacity);
    let mut bytes = Vec::new();
    table.start().encode(&mut bytes);
    let (mut count, mut checksum) = (1, 0);
    for (name, value) in messages.flat_map(|message| &message.fields) {
        if let Some(instruction) = table.instruction(name, value) {
            instruction.encode(&mut bytes);
            count += 1;
            checksum += entry_checksum(name.as_bytes(), value.as_bytes());
        }
    }
    Instructions {
        bytes,
        count,
        checksum,
    }
}

// The decoder's table, shared by every variant.

#[derive(Default)]
pub struct Entry {
    name: Vec<u8>,
    value: Vec<u8>,
}

/// The decoder's dynamic table. Evicted entries keep their buffers for the
/// next insertion.
pub struct Table {
    maximum: usize,
    capacity: usize,
    size: usize,
    /// Oldest first.
    entries: VecDeque<Entry>,
    spare: Vec<Entry>,
    /// The sum of the checksums of every entry inserted.
    pub checksum: u64,
}
impl Table {
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum,
            capacity: 0,
            size: 0,
            entries: VecDeque::new(),
            spare: Vec::new(),
            checksum: 0,
        }
    }
    fn evict_to(&mut self, size: usize) {
        while self.size > size {
            let entry = self.entries.pop_front().unwrap();
            self.size -= entry_size(&entry.name, &entry.value);
            self.spare.push(entry);
        }
    }
    fn set_capacity(&mut self, capacity: usize) -> io::Result<()> {
        if capacity > self.maximum {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.capacity = capacity;
        self.evict_to(capacity);
        Ok(())
    }
    fn relative(&self, index: usize) -> io::Result<&Entry> {
        self.entries
            .len()
            .checked_sub(index + 1)
            .and_then(|index| self.entries.get(index))
            .ok_or_else(|| io::ErrorKind::InvalidData.into())
    }
    /// An empty entry to decode into.
    fn entry(&mut self) -> Entry {
        let mut entry = self.spare.pop().unwrap_or_default();
        entry.name.clear();
        entry.value.clear();
        entry
    }
    /// An entry named after a static or dynamic table entry.
    fn named(&mut self, is_static: bool, index: usize) -> io::Result<Entry> {
        let mut entry = self.entry();
        if is_static {
            let (name, _) = qpack::entry(index).map_err(invalid)?;
            entry.name.extend_from_slice(name.as_bytes());
        } else {
            entry.name.extend_from_slice(&self.relative(index)?.name);
        }
        Ok(entry)
    }
    fn duplicate(&mut self, index: usize) -> io::Result<()> {
        let mut entry = self.named(false, index)?;
        entry.value.extend_from_slice(&self.relative(index)?.value);
        self.insert(entry)
    }
    fn insert(&mut self, entry: Entry) -> io::Result<()> {
        let size = entry_size(&entry.name, &entry.value);
        if size > self.capacity {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.evict_to(self.capacity - size);
        self.checksum += entry_checksum(&entry.name, &entry.value);
        self.size += size;
        self.entries.push_back(entry);
        Ok(())
    }
}

/// What the first byte of an instruction says.
enum Start {
    /// Insert with Name Reference; the name index follows with a 6-bit
    /// prefix.
    NameReference { is_static: bool },
    /// Insert with Literal Name; the name follows with a 5-bit prefix.
    LiteralName,
    /// Set Dynamic Table Capacity, with a 5-bit prefix.
    Capacity,
    /// Duplicate, with a 5-bit prefix.
    Duplicate,
}
fn start(first: u8) -> Start {
    if first & 0x80 != 0 {
        Start::NameReference {
            is_static: first & 0x40 != 0,
        }
    } else if first & 0x40 != 0 {
        Start::LiteralName
    } else if first & 0x20 != 0 {
        Start::Capacity
    } else {
        Start::Duplicate
    }
}

/// A prefix integer read a byte at a time.
enum Integer {
    Done(usize),
    More { value: usize, shift: u32 },
}
impl Integer {
    fn first(byte: u8, prefix: u8) -> Self {
        let limit = (1usize << prefix) - 1;
        match byte as usize & limit {
            value if value < limit => Integer::Done(value),
            value => Integer::More { value, shift: 0 },
        }
    }
    fn next(self, byte: u8) -> io::Result<Self> {
        let Integer::More { value, shift } = self else {
            unreachable!("a complete integer")
        };
        if shift > 28 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let value = value + (((byte & 0x7f) as usize) << shift);
        Ok(if byte & 0x80 == 0 {
            Integer::Done(value)
        } else {
            Integer::More {
                value,
                shift: shift + 7,
            }
        })
    }
}

/// How a string's bytes become its value.
enum StringDecoder {
    Raw,
    Huffman(Huffman),
}
impl StringDecoder {
    fn new(first: u8, prefix: u8) -> Self {
        if first & (1 << prefix) != 0 {
            StringDecoder::Huffman(Huffman::default())
        } else {
            StringDecoder::Raw
        }
    }
    fn feed(&mut self, piece: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
        match self {
            StringDecoder::Raw => output.extend_from_slice(piece),
            StringDecoder::Huffman(huffman) => huffman.feed(piece, output).map_err(invalid)?,
        }
        Ok(())
    }
    fn finish(self) -> io::Result<()> {
        match self {
            StringDecoder::Raw => Ok(()),
            StringDecoder::Huffman(huffman) => huffman.finish().map_err(invalid),
        }
    }
}

fn eof() -> io::Error {
    io::ErrorKind::UnexpectedEof.into()
}

/// Where the bytes of a string are decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Strings {
    /// In the stream buffer, while it is held.
    InPlace,
    /// Taken out of the buffer without copying, after it is released.
    Taken,
}

// Ordinary async: every byte or piece read locks.

async fn plain_byte<S: PollConsume + ?Sized>(source: &mut S) -> io::Result<Option<u8>> {
    let mut byte = None;
    poll_fn(|cx| source.poll_consume(cx, 1, &mut |bytes: &[u8]| byte = Some(bytes[0]))).await?;
    Ok(byte)
}

async fn plain_integer<S: PollConsume + ?Sized>(
    source: &mut S,
    first: u8,
    prefix: u8,
) -> io::Result<usize> {
    let mut integer = Integer::first(first, prefix);
    loop {
        match integer {
            Integer::Done(value) => return Ok(value),
            Integer::More { .. } => {
                let byte = plain_byte(source).await?.ok_or_else(eof)?;
                integer = integer.next(byte)?;
            }
        }
    }
}

/// Read the bytes of a string whose first byte is `first`.
async fn plain_string_bytes<S: PollConsume + ?Sized>(
    source: &mut S,
    first: u8,
    prefix: u8,
    output: &mut Vec<u8>,
    strings: Strings,
) -> io::Result<()> {
    let mut decoder = StringDecoder::new(first, prefix);
    let mut remaining = plain_integer(source, first, prefix).await?;
    while remaining > 0 {
        let count = match strings {
            Strings::InPlace => {
                let mut fed = Ok(());
                let count = poll_fn(|cx| {
                    source.poll_consume(cx, remaining, &mut |piece: &[u8]| {
                        fed = decoder.feed(piece, output)
                    })
                })
                .await?;
                fed?;
                count
            }
            Strings::Taken => {
                let piece = poll_fn(|cx| source.poll_take(cx, remaining)).await?;
                decoder.feed(&piece, output)?;
                piece.len()
            }
        };
        if count == 0 {
            return Err(eof());
        }
        remaining -= count;
    }
    decoder.finish()
}

async fn plain_string<S: PollConsume + ?Sized>(
    source: &mut S,
    prefix: u8,
    output: &mut Vec<u8>,
    strings: Strings,
) -> io::Result<()> {
    let first = plain_byte(source).await?.ok_or_else(eof)?;
    plain_string_bytes(source, first, prefix, output, strings).await
}

/// Read and apply one instruction; `false` at the end of the stream.
async fn plain_instruction<S: PollConsume + ?Sized>(
    source: &mut S,
    table: &mut Table,
    strings: Strings,
) -> io::Result<bool> {
    let Some(first) = plain_byte(source).await? else {
        return Ok(false);
    };
    match start(first) {
        Start::NameReference { is_static } => {
            let index = plain_integer(source, first, 6).await?;
            let mut entry = table.named(is_static, index)?;
            plain_string(source, 7, &mut entry.value, strings).await?;
            table.insert(entry)?;
        }
        Start::LiteralName => {
            let mut entry = table.entry();
            plain_string_bytes(source, first, 5, &mut entry.name, strings).await?;
            plain_string(source, 7, &mut entry.value, strings).await?;
            table.insert(entry)?;
        }
        Start::Capacity => {
            let capacity = plain_integer(source, first, 5).await?;
            table.set_capacity(capacity)?;
        }
        Start::Duplicate => {
            let index = plain_integer(source, first, 5).await?;
            table.duplicate(index)?;
        }
    }
    Ok(true)
}

// Calls: the same steps as operations. A piece of a string taken out of the
// buffer is decoded after a release point.

#[call]
async fn read_byte(io: &mut impl ReceiverAccess<Family: Consume>) -> io::Result<Option<u8>> {
    let mut byte = None;
    io.consume(1, &mut |bytes: &[u8]| byte = Some(bytes[0]))
        .await?;
    Ok(byte)
}

#[call]
async fn read_integer(
    io: &mut impl ReceiverAccess<Family: Consume>,
    first: u8,
    prefix: u8,
) -> io::Result<usize> {
    let mut integer = Integer::first(first, prefix);
    loop {
        match integer {
            Integer::Done(value) => return Ok(value),
            Integer::More { .. } => {
                let byte = read_byte(io).await?.ok_or_else(eof)?;
                integer = integer.next(byte)?;
            }
        }
    }
}

/// Read the bytes of a string whose first byte is `first`.
#[call]
async fn read_string_bytes(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    first: u8,
    prefix: u8,
    output: &mut Vec<u8>,
    strings: Strings,
) -> io::Result<()> {
    let mut decoder = StringDecoder::new(first, prefix);
    let mut remaining = read_integer(io, first, prefix).await?;
    while remaining > 0 {
        let count = match strings {
            Strings::InPlace => {
                let mut fed = Ok(());
                let count = io
                    .consume(remaining, &mut |piece: &[u8]| {
                        fed = decoder.feed(piece, output)
                    })
                    .await?;
                fed?;
                count
            }
            Strings::Taken => {
                let piece = io.take(remaining).await?;
                io.release_view().await;
                decoder.feed(&piece, output)?;
                piece.len()
            }
        };
        if count == 0 {
            return Err(eof());
        }
        remaining -= count;
    }
    decoder.finish()
}

#[call]
async fn read_string(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    prefix: u8,
    output: &mut Vec<u8>,
    strings: Strings,
) -> io::Result<()> {
    let first = read_byte(io).await?.ok_or_else(eof)?;
    read_string_bytes(io, first, prefix, output, strings).await
}

#[call]
async fn read_instruction(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    table: &mut Table,
    strings: Strings,
) -> io::Result<bool> {
    let Some(first) = read_byte(io).await? else {
        return Ok(false);
    };
    match start(first) {
        Start::NameReference { is_static } => {
            let index = read_integer(io, first, 6).await?;
            let mut entry = table.named(is_static, index)?;
            read_string(io, 7, &mut entry.value, strings).await?;
            table.insert(entry)?;
        }
        Start::LiteralName => {
            let mut entry = table.entry();
            read_string_bytes(io, first, 5, &mut entry.name, strings).await?;
            read_string(io, 7, &mut entry.value, strings).await?;
            table.insert(entry)?;
        }
        Start::Capacity => {
            let capacity = read_integer(io, first, 5).await?;
            table.set_capacity(capacity)?;
        }
        Start::Duplicate => {
            let index = read_integer(io, first, 5).await?;
            table.duplicate(index)?;
        }
    }
    Ok(true)
}

/// Read and apply every instruction.
#[call]
pub async fn read_instructions(
    io: &mut impl ReceiverAccess<Family: Consume + Take>,
    table: &mut Table,
    strings: Strings,
) -> io::Result<()> {
    while read_instruction(io, table, strings).await? {}
    Ok(())
}

// The codec: under the lock, find where the complete instructions end by
// parsing only their prefix integers, and split them off together; decode
// them after unlocking.

/// The length of the instruction at the start of `bytes`, or `None` if it is
/// incomplete.
fn instruction_length(bytes: &[u8]) -> io::Result<Option<usize>> {
    /// Skip a prefix integer, returning its value.
    fn integer(bytes: &[u8], position: &mut usize, prefix: u8) -> io::Result<Option<usize>> {
        let Some(&first) = bytes.get(*position) else {
            return Ok(None);
        };
        *position += 1;
        let mut integer = Integer::first(first, prefix);
        loop {
            match integer {
                Integer::Done(value) => return Ok(Some(value)),
                Integer::More { .. } => {
                    let Some(&byte) = bytes.get(*position) else {
                        return Ok(None);
                    };
                    *position += 1;
                    integer = integer.next(byte)?;
                }
            }
        }
    }
    fn string(bytes: &[u8], position: &mut usize, prefix: u8) -> io::Result<Option<()>> {
        let Some(length) = integer(bytes, position, prefix)? else {
            return Ok(None);
        };
        *position += length;
        Ok((*position <= bytes.len()).then_some(()))
    }
    let Some(&first) = bytes.first() else {
        return Ok(None);
    };
    let mut position = 0;
    let complete = match start(first) {
        Start::NameReference { .. } => {
            integer(bytes, &mut position, 6)?.is_some()
                && string(bytes, &mut position, 7)?.is_some()
        }
        Start::LiteralName => {
            string(bytes, &mut position, 5)?.is_some() && string(bytes, &mut position, 7)?.is_some()
        }
        Start::Capacity | Start::Duplicate => integer(bytes, &mut position, 5)?.is_some(),
    };
    Ok(complete.then_some(position))
}

/// Hands out every complete instruction that is buffered, as one piece.
#[derive(Default)]
pub struct InstructionDecoder;
impl Decoder for InstructionDecoder {
    type Item = BytesMut;
    type Error = io::Error;
    fn decode(&mut self, source: &mut BytesMut) -> io::Result<Option<BytesMut>> {
        let mut end = 0;
        while let Some(length) = instruction_length(&source[end..])? {
            end += length;
        }
        Ok((end > 0).then(|| source.split_to(end)))
    }
}

/// Apply complete instructions.
fn apply(bytes: &[u8], table: &mut Table) -> io::Result<()> {
    let mut position = 0;
    while position < bytes.len() {
        let first = bytes[position];
        match start(first) {
            Start::NameReference { is_static } => {
                let index = decode_integer(bytes, &mut position, 6).map_err(invalid)?;
                let mut entry = table.named(is_static, index)?;
                decode_string(bytes, &mut position, 7, &mut entry.value).map_err(invalid)?;
                table.insert(entry)?;
            }
            Start::LiteralName => {
                let mut entry = table.entry();
                decode_string(bytes, &mut position, 5, &mut entry.name).map_err(invalid)?;
                decode_string(bytes, &mut position, 7, &mut entry.value).map_err(invalid)?;
                table.insert(entry)?;
            }
            Start::Capacity => {
                let capacity = decode_integer(bytes, &mut position, 5).map_err(invalid)?;
                table.set_capacity(capacity)?;
            }
            Start::Duplicate => {
                let index = decode_integer(bytes, &mut position, 5).map_err(invalid)?;
                table.duplicate(index)?;
            }
        }
    }
    Ok(())
}

// Sending.

/// A prefix integer, encoded on the stack.
fn integer_bytes(value: usize, prefix: u8, flags: u8) -> ([u8; 11], usize) {
    let mut bytes = [0; 11];
    let limit = (1usize << prefix) - 1;
    if value < limit {
        bytes[0] = flags | value as u8;
        return (bytes, 1);
    }
    bytes[0] = flags | limit as u8;
    let (mut rest, mut length) = (value - limit, 1);
    while rest >= 128 {
        bytes[length] = rest as u8 | 0x80;
        rest >>= 7;
        length += 1;
    }
    bytes[length] = rest as u8;
    (bytes, length + 1)
}

/// The room on the stack for a piece of a Huffman-coded string.
const HUFFMAN_PIECE: usize = 64;

// Ordinary async: every write locks.

async fn plain_write_integer<S: PollWrite + ?Sized>(
    sink: &mut S,
    value: usize,
    prefix: u8,
    flags: u8,
) -> io::Result<()> {
    let (bytes, length) = integer_bytes(value, prefix, flags);
    plain_write_all(sink, &bytes[..length]).await
}

/// Write a string, Huffman-coded when that is shorter.
async fn plain_write_string<S: PollWrite + ?Sized>(
    sink: &mut S,
    value: &[u8],
    prefix: u8,
    flags: u8,
) -> io::Result<()> {
    let coded = huffman_length(value);
    if coded >= value.len() {
        plain_write_integer(sink, value.len(), prefix, flags).await?;
        return plain_write_all(sink, value).await;
    }
    plain_write_integer(sink, coded, prefix, flags | (1 << prefix)).await?;
    let (mut encoder, mut piece) = (HuffmanEncoder::new(value), [0; HUFFMAN_PIECE]);
    loop {
        match encoder.fill(&mut piece) {
            0 => return Ok(()),
            filled => plain_write_all(sink, &piece[..filled]).await?,
        }
    }
}

async fn plain_write_instruction<S: PollWrite + ?Sized>(
    sink: &mut S,
    instruction: Instruction<'_>,
) -> io::Result<()> {
    match instruction {
        Instruction::Capacity(capacity) => plain_write_integer(sink, capacity, 5, 0x20).await,
        Instruction::Duplicate(index) => plain_write_integer(sink, index, 5, 0x00).await,
        Instruction::NameReference {
            is_static,
            index,
            value,
        } => {
            let flags = if is_static { 0xc0 } else { 0x80 };
            plain_write_integer(sink, index, 6, flags).await?;
            plain_write_string(sink, value.as_bytes(), 7, 0x00).await
        }
        Instruction::LiteralName { name, value } => {
            plain_write_string(sink, name.as_bytes(), 5, 0x40).await?;
            plain_write_string(sink, value.as_bytes(), 7, 0x00).await
        }
    }
}

// Calls: the same steps as operations, with a release point before choosing
// the instruction for each field.

#[call]
async fn write_integer(
    io: &mut impl ReceiverAccess<Family: Write>,
    value: usize,
    prefix: u8,
    flags: u8,
) -> io::Result<()> {
    let (bytes, length) = integer_bytes(value, prefix, flags);
    write_all(io, &bytes[..length]).await
}

/// Write a string, Huffman-coded when that is shorter.
#[call]
async fn write_string(
    io: &mut impl ReceiverAccess<Family: Write>,
    value: &[u8],
    prefix: u8,
    flags: u8,
) -> io::Result<()> {
    let coded = huffman_length(value);
    if coded >= value.len() {
        write_integer(io, value.len(), prefix, flags).await?;
        return write_all(io, value).await;
    }
    write_integer(io, coded, prefix, flags | (1 << prefix)).await?;
    let (mut encoder, mut piece) = (HuffmanEncoder::new(value), [0; HUFFMAN_PIECE]);
    loop {
        match encoder.fill(&mut piece) {
            0 => return Ok(()),
            filled => write_all(io, &piece[..filled]).await?,
        }
    }
}

#[call]
async fn write_instruction(
    io: &mut impl ReceiverAccess<Family: Write>,
    instruction: Instruction<'_>,
) -> io::Result<()> {
    match instruction {
        Instruction::Capacity(capacity) => write_integer(io, capacity, 5, 0x20).await,
        Instruction::Duplicate(index) => write_integer(io, index, 5, 0x00).await,
        Instruction::NameReference {
            is_static,
            index,
            value,
        } => {
            let flags = if is_static { 0xc0 } else { 0x80 };
            write_integer(io, index, 6, flags).await?;
            write_string(io, value.as_bytes(), 7, 0x00).await
        }
        Instruction::LiteralName { name, value } => {
            write_string(io, name.as_bytes(), 5, 0x40).await?;
            write_string(io, value.as_bytes(), 7, 0x00).await
        }
    }
}

/// Write the instructions for every field section of connection `slot` of
/// `streams`. It owns the messages, so the execution borrows nothing but the
/// executor.
#[call]
pub async fn write_instructions(
    io: &mut impl ReceiverAccess<Family: Write>,
    messages: Arc<Vec<Message>>,
    slot: usize,
    streams: usize,
    capacity: usize,
) -> io::Result<()> {
    let mut table = EncoderTable::new(capacity);
    write_instruction(io, table.start()).await?;
    let sections = messages.iter().skip(slot).step_by(streams);
    for (name, value) in sections.flat_map(|message| &message.fields) {
        io.release_view().await;
        if let Some(instruction) = table.instruction(name, value) {
            write_instruction(io, instruction).await?;
        }
    }
    Ok(())
}

/// Encodes instructions into the codec's buffer.
#[derive(Default)]
pub struct InstructionEncoder {
    encoded: Vec<u8>,
}
impl Encoder<Instruction<'_>> for InstructionEncoder {
    type Error = io::Error;
    fn encode(&mut self, instruction: Instruction<'_>, output: &mut BytesMut) -> io::Result<()> {
        self.encoded.clear();
        instruction.encode(&mut self.encoded);
        output.extend_from_slice(&self.encoded);
        Ok(())
    }
}

/// Decodes and applies the instructions of each stream.
struct InstructionCheck {
    received: Vec<BytesMut>,
    tables: Vec<Table>,
}
impl Check for InstructionCheck {
    fn take(&mut self, index: usize, packet: &[u8]) {
        let received = &mut self.received[index];
        received.extend_from_slice(packet);
        if let Some(ready) = InstructionDecoder.decode(received).unwrap() {
            apply(&ready, &mut self.tables[index]).unwrap();
        }
    }
    fn finish(self) -> u64 {
        assert!(
            self.received.iter().all(BytesMut::is_empty),
            "incomplete instruction"
        );
        self.tables.iter().map(|table| table.checksum).sum()
    }
}

// Runs.

#[derive(Clone)]
pub struct Input {
    pub messages: Arc<Vec<Message>>,
    pub streams: usize,
    pub pace: Pace,
    pub trickle: bool,
    pub update: usize,
    pub capacity: usize,
    /// Each connection's encoder stream.
    pub bytes: Arc<Vec<Vec<u8>>>,
    pub instructions: usize,
    pub checksum: u64,
}
impl Input {
    /// One encoder stream for each of `streams` connections, carrying every
    /// `streams`-th message.
    pub fn new(
        messages: Arc<Vec<Message>>,
        streams: usize,
        pace: Pace,
        trickle: bool,
        update: usize,
        capacity: usize,
    ) -> Self {
        let encoded: Vec<_> = (0..streams)
            .map(|slot| encode(messages.iter().skip(slot).step_by(streams), capacity))
            .collect();
        Self {
            messages,
            streams,
            pace,
            trickle,
            update,
            capacity,
            instructions: encoded.iter().map(|e| e.count).sum(),
            checksum: encoded.iter().map(|e| e.checksum).sum(),
            bytes: Arc::new(encoded.into_iter().map(|e| e.bytes).collect()),
        }
    }
    pub fn size(&self) -> usize {
        self.bytes.iter().map(Vec::len).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecvVariant {
    /// Decoding strings in place, inside the lock of each leaf.
    PlainLockPerLeaf,
    /// One call reads every instruction, keeping the view until the end of
    /// each poll and decoding strings in place.
    Call,
    /// The same, taking each piece of a string out of the buffer and
    /// releasing the view before decoding it.
    CallRelease,
    /// Lock, split off every complete instruction, and apply them after
    /// unlocking.
    Codec,
}
pub const RECVS: [RecvVariant; 4] = [
    RecvVariant::PlainLockPerLeaf,
    RecvVariant::Call,
    RecvVariant::CallRelease,
    RecvVariant::Codec,
];

async fn receive<L: Lock<Value = RecvStream>>(
    variant: RecvVariant,
    stream: &L,
    counters: &Counters,
    capacity: usize,
) -> u64 {
    let mut table = Table::new(capacity);
    // Decoding in place is faster for both ways that can (by 5–10% when
    // measured); releasing the view needs the bytes taken out.
    let strings = match variant {
        RecvVariant::CallRelease => Strings::Taken,
        _ => Strings::InPlace,
    };
    match variant {
        RecvVariant::PlainLockPerLeaf => {
            let mut source = LockPerLeaf::new(stream, &counters.locks);
            while plain_instruction(&mut source, &mut table, strings)
                .await
                .unwrap()
            {}
        }
        RecvVariant::Call | RecvVariant::CallRelease => {
            let release = match variant {
                RecvVariant::CallRelease => Release::Always,
                _ => Release::Never,
            };
            let mut executor = Shared {
                lock: stream,
                stats: &counters.locks,
                release,
            };
            executor
                .read_instructions_unpin(&mut table, strings)
                .await
                .unwrap();
        }
        RecvVariant::Codec => {
            let mut codec = InstructionDecoder;
            let mut ended = false;
            while !ended {
                let mut attempt = PollLock::new(stream, &counters.locks);
                let ready = poll_fn(|cx| {
                    let mut stream = ready!(attempt.poll_lock(cx));
                    let ready = codec.decode(stream.buffer()).unwrap();
                    if let Some(ready) = &ready {
                        stream.consumed(ready.len());
                    }
                    // Ask for a wakeup before unlocking, so that the next
                    // acquisition waits for new bytes, or learn that the
                    // stream has ended.
                    if let Poll::Ready(more) = stream.poll_more(cx) {
                        assert!(!more.unwrap());
                        assert!(stream.buffer().is_empty(), "incomplete instruction");
                        ended = true;
                        return Poll::Ready(ready);
                    }
                    match ready {
                        Some(ready) => Poll::Ready(Some(ready)),
                        None => Poll::Pending,
                    }
                })
                .await;
                if let Some(ready) = ready {
                    apply(&ready, &mut table).unwrap();
                }
                if !ended {
                    woken().await;
                }
            }
        }
    }
    table.checksum
}

struct RunRecv<'r> {
    runtime: &'r Runtime,
    variant: RecvVariant,
    input: Input,
    holds: bool,
}
impl PerLock<RecvStream> for RunRecv<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = RecvStream>>(self) -> Sample {
        let RunRecv {
            runtime,
            variant,
            input,
            holds,
        } = self;
        runtime.block_on(async move {
            let conn = recv_conn::<L>(input.streams, input.update);
            let counters = Arc::new(Counters::new(holds));
            let start = Instant::now();
            // Joined below; it ends after closing every stream.
            let deliverer = tokio::spawn(deliver(
                conn.clone(),
                input.bytes.clone(),
                input.pace,
                input.trickle,
                counters.clone(),
            ));
            let capacity = input.capacity;
            let checksum = per_stream(input.streams, |index| {
                let (conn, counters) = (conn.clone(), counters.clone());
                async move {
                    counted(
                        &counters.polls,
                        receive(variant, &conn.streams[index], &counters, capacity),
                    )
                    .await
                }
            })
            .await;
            let elapsed = start.elapsed();
            let waits = deliverer.await.unwrap();
            assert_eq!(checksum, input.checksum, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

pub fn run_recv(
    runtime: &Runtime,
    variant: RecvVariant,
    lock: LockKind,
    input: Input,
    holds: bool,
) -> Sample {
    with_lock(
        lock,
        RunRecv {
            runtime,
            variant,
            input,
            holds,
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SendVariant {
    /// Writing each integer and each piece of a string inside the lock of
    /// each leaf.
    PlainLockPerLeaf,
    /// One call writes every instruction, keeping the view until the end of
    /// each poll.
    Call,
    /// The same, releasing the view at every release point.
    CallRelease,
    /// `FramedWrite` over an `AsyncWrite` of the stream, flushed after each
    /// field section's instructions.
    Codec,
}
pub const SENDS: [SendVariant; 4] = [
    SendVariant::PlainLockPerLeaf,
    SendVariant::Call,
    SendVariant::CallRelease,
    SendVariant::Codec,
];

async fn send<L: Lock<Value = SendStream>>(
    variant: SendVariant,
    stream: &L,
    slot: usize,
    counters: &Counters,
    input: &Input,
) {
    let sections = || input.messages.iter().skip(slot).step_by(input.streams);
    let mut table = EncoderTable::new(input.capacity);
    match variant {
        SendVariant::PlainLockPerLeaf => {
            let mut sink = LockPerLeaf::new(stream, &counters.locks);
            plain_write_instruction(&mut sink, table.start())
                .await
                .unwrap();
            for (name, value) in sections().flat_map(|message| &message.fields) {
                if let Some(instruction) = table.instruction(name, value) {
                    plain_write_instruction(&mut sink, instruction)
                        .await
                        .unwrap();
                }
            }
        }
        SendVariant::Call | SendVariant::CallRelease => {
            let release = match variant {
                SendVariant::CallRelease => Release::Always,
                _ => Release::Never,
            };
            let mut executor = Shared {
                lock: stream,
                stats: &counters.locks,
                release,
            };
            executor
                .write_instructions_unpin(
                    input.messages.clone(),
                    slot,
                    input.streams,
                    input.capacity,
                )
                .await
                .unwrap();
        }
        SendVariant::Codec => {
            let writer = StreamWriter {
                attempt: PollLock::new(stream, &counters.locks),
            };
            let mut sink = FramedWrite::new(writer, InstructionEncoder::default());
            Pin::new(&mut sink).start_send(table.start()).unwrap();
            for message in sections() {
                for (name, value) in &message.fields {
                    if let Some(instruction) = table.instruction(name, value) {
                        poll_fn(|cx| Pin::new(&mut sink).poll_ready(cx))
                            .await
                            .unwrap();
                        Pin::new(&mut sink).start_send(instruction).unwrap();
                    }
                }
                poll_fn(|cx| Pin::new(&mut sink).poll_flush(cx))
                    .await
                    .unwrap();
            }
        }
    }
}

struct RunSend<'r> {
    runtime: &'r Runtime,
    variant: SendVariant,
    input: Input,
    holds: bool,
}
impl PerLock<SendStream> for RunSend<'_> {
    type Output = Sample;
    fn run<L: Lock<Value = SendStream>>(self) -> Sample {
        let RunSend {
            runtime,
            variant,
            input,
            holds,
        } = self;
        runtime.block_on(async move {
            let conn = send_conn::<L>(input.streams, input.update);
            let counters = Arc::new(Counters::new(holds));
            let check = InstructionCheck {
                received: (0..input.streams).map(|_| BytesMut::new()).collect(),
                tables: (0..input.streams)
                    .map(|_| Table::new(input.capacity))
                    .collect(),
            };
            let start = Instant::now();
            // Joined below; it ends once every stream is closed and empty.
            let drainer = tokio::spawn(drain(conn.clone(), input.pace, counters.clone(), check));
            per_stream(input.streams, |slot| {
                let (conn, counters, input) = (conn.clone(), counters.clone(), input.clone());
                async move {
                    let stream = &conn.streams[slot];
                    counted(
                        &counters.polls,
                        send(variant, stream, slot, &counters, &input),
                    )
                    .await;
                    close(stream, &counters).await;
                    0
                }
            })
            .await;
            let (checksum, waits) = drainer.await.unwrap();
            let elapsed = start.elapsed();
            assert_eq!(checksum, input.checksum, "{variant:?}");
            Sample::new(elapsed, &counters, waits)
        })
    }
}

pub fn run_send(
    runtime: &Runtime,
    variant: SendVariant,
    lock: LockKind,
    input: Input,
    holds: bool,
) -> Sample {
    with_lock(
        lock,
        RunSend {
            runtime,
            variant,
            input,
            holds,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload;

    #[test]
    fn every_kind_of_instruction_appears_and_applies() {
        let messages = workload::messages(1);
        let instructions = encode(messages.iter(), 4096);
        let (mut kinds, mut position) = ([0usize; 5], 0);
        let bytes = &instructions.bytes;
        while let Some(length) = instruction_length(&bytes[position..]).unwrap() {
            let slot = match start(bytes[position]) {
                Start::NameReference { is_static: true } => 0,
                Start::NameReference { is_static: false } => 1,
                Start::LiteralName => 2,
                Start::Capacity => 3,
                Start::Duplicate => 4,
            };
            kinds[slot] += 1;
            position += length;
        }
        assert_eq!(position, bytes.len());
        assert!(kinds.iter().all(|&count| count > 0), "{kinds:?}");
        assert_eq!(kinds.iter().sum::<usize>(), instructions.count);
        let mut table = Table::new(4096);
        apply(bytes, &mut table).unwrap();
        assert_eq!(table.checksum, instructions.checksum);
        // Every split point leaves an incomplete instruction undecided.
        let mut decoder = InstructionDecoder;
        for split in 1..64 {
            let mut source = BytesMut::from(&bytes[..split]);
            let ready = decoder.decode(&mut source).unwrap().unwrap_or_default();
            assert_eq!(ready.len() + source.len(), split);
            assert_eq!(instruction_length(&source).unwrap(), None);
        }
    }
}
