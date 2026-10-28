//! HPACK (RFC 7541) and QPACK (RFC 9204) field sections that use only the
//! static table, with Huffman-coded strings.
//!
//! The encoders never add entries to a dynamic table, which is what QPACK
//! does when the decoder advertises no table capacity (the default) and what
//! an HPACK encoder may choose to do. The decoders accept exactly the
//! representations that refer to the static table or carry literals; a
//! reference to a dynamic table is reported as unsupported.
use crate::tables::{HPACK_STATIC, HUFFMAN, QPACK_STATIC};
use std::{collections::HashMap, sync::OnceLock};

#[derive(Debug, PartialEq)]
pub enum FieldError {
    Truncated,
    Huffman,
    /// A representation that needs a dynamic table.
    Unsupported,
    Index,
}

// Integers (RFC 7541 Section 5.1, used by both).

pub fn encode_integer(value: usize, prefix: u8, flags: u8, output: &mut Vec<u8>) {
    let limit = (1usize << prefix) - 1;
    if value < limit {
        output.push(flags | value as u8);
        return;
    }
    output.push(flags | limit as u8);
    let mut rest = value - limit;
    while rest >= 128 {
        output.push(rest as u8 | 0x80);
        rest >>= 7;
    }
    output.push(rest as u8);
}

pub fn decode_integer(input: &[u8], position: &mut usize, prefix: u8) -> Result<usize, FieldError> {
    let limit = (1usize << prefix) - 1;
    let first = *input.get(*position).ok_or(FieldError::Truncated)? as usize & limit;
    *position += 1;
    if first < limit {
        return Ok(first);
    }
    let mut value = limit;
    let mut shift = 0;
    loop {
        let byte = *input.get(*position).ok_or(FieldError::Truncated)?;
        *position += 1;
        value += ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 28 {
            return Err(FieldError::Truncated);
        }
    }
}

// Huffman (RFC 7541 Section 5.2 and Appendix B).

pub fn huffman_length(bytes: &[u8]) -> usize {
    let bits: usize = bytes.iter().map(|&b| HUFFMAN[b as usize].1 as usize).sum();
    bits.div_ceil(8)
}

fn huffman_encode(bytes: &[u8], output: &mut Vec<u8>) {
    let mut pending = 0u64;
    let mut count = 0;
    for &byte in bytes {
        let (code, length) = HUFFMAN[byte as usize];
        pending = (pending << length) | code as u64;
        count += length as u32;
        while count >= 8 {
            count -= 8;
            output.push((pending >> count) as u8);
        }
    }
    if count > 0 {
        // Pad with the most significant bits of EOS, which are all ones.
        output.push(((pending << (8 - count)) | (0xff >> count)) as u8);
    }
}

/// Huffman-codes a string a piece at a time, into whatever room the caller
/// has.
pub struct HuffmanEncoder<'a> {
    input: &'a [u8],
    pending: u64,
    count: u32,
}
impl<'a> HuffmanEncoder<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pending: 0,
            count: 0,
        }
    }
    /// Fill as much of `output` as there is coded output for; 0 once done.
    pub fn fill(&mut self, output: &mut [u8]) -> usize {
        let mut filled = 0;
        while filled < output.len() {
            if self.count >= 8 {
                self.count -= 8;
                output[filled] = (self.pending >> self.count) as u8;
                filled += 1;
            } else if let Some((&byte, rest)) = self.input.split_first() {
                let (code, length) = HUFFMAN[byte as usize];
                self.pending = (self.pending << length) | code as u64;
                self.count += length as u32;
                self.input = rest;
            } else if self.count > 0 {
                // Pad as `huffman_encode` does.
                output[filled] = ((self.pending << (8 - self.count)) | (0xff >> self.count)) as u8;
                self.count = 0;
                filled += 1;
            } else {
                break;
            }
        }
        filled
    }
}

/// One step of the decoder: the state after four bits, and the symbol they
/// completed, if any.
#[derive(Clone, Copy, Default)]
struct Step {
    next: u16,
    symbol: Option<u8>,
    failed: bool,
}

struct HuffmanDecoder {
    /// Indexed by state * 16 + nibble. State 0 is the root of the code tree.
    steps: Vec<Step>,
    /// Whether decoding may end in each state: at the root, or within at most
    /// seven bits of padding (a prefix of EOS).
    accepting: Vec<bool>,
}

fn huffman_decoder() -> &'static HuffmanDecoder {
    static DECODER: OnceLock<HuffmanDecoder> = OnceLock::new();
    DECODER.get_or_init(|| {
        // Internal nodes of the code tree; a child is a node or a symbol.
        #[derive(Clone, Copy)]
        enum Child {
            Missing,
            Node(usize),
            Symbol(u16),
        }
        let mut nodes = vec![[Child::Missing; 2]];
        let mut depth = vec![0u8];
        let mut ones = vec![true];
        for (symbol, &(code, length)) in HUFFMAN.iter().enumerate() {
            let mut node = 0;
            for bit_index in (0..length).rev() {
                let bit = (code >> bit_index) as usize & 1;
                if bit_index == 0 {
                    nodes[node][bit] = Child::Symbol(symbol as u16);
                    break;
                }
                node = match nodes[node][bit] {
                    Child::Node(next) => next,
                    Child::Missing => {
                        nodes.push([Child::Missing; 2]);
                        depth.push(depth[node] + 1);
                        ones.push(ones[node] && bit == 1);
                        let next = nodes.len() - 1;
                        nodes[node][bit] = Child::Node(next);
                        next
                    }
                    Child::Symbol(_) => unreachable!("prefix code"),
                };
            }
        }
        let mut steps = vec![Step::default(); nodes.len() * 16];
        for state in 0..nodes.len() {
            for nibble in 0..16 {
                let mut node = state;
                let mut step = Step::default();
                for bit_index in (0..4).rev() {
                    match nodes[node][(nibble >> bit_index) & 1] {
                        Child::Node(next) => node = next,
                        Child::Symbol(256) | Child::Missing => {
                            step.failed = true;
                            break;
                        }
                        Child::Symbol(symbol) => {
                            step.symbol = Some(symbol as u8);
                            node = 0;
                        }
                    }
                }
                step.next = node as u16;
                steps[state * 16 + nibble] = step;
            }
        }
        let accepting = (0..nodes.len())
            .map(|node| node == 0 || (ones[node] && depth[node] <= 7))
            .collect();
        HuffmanDecoder { steps, accepting }
    })
}

/// A Huffman-coded string decoded piece by piece.
#[derive(Default)]
pub struct Huffman {
    state: usize,
}
impl Huffman {
    pub fn feed(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<(), FieldError> {
        let decoder = huffman_decoder();
        for &byte in input {
            for nibble in [byte >> 4, byte & 0x0f] {
                let step = decoder.steps[self.state * 16 + nibble as usize];
                if step.failed {
                    return Err(FieldError::Huffman);
                }
                if let Some(symbol) = step.symbol {
                    output.push(symbol);
                }
                self.state = step.next as usize;
            }
        }
        Ok(())
    }
    /// Check that the string ended with valid padding.
    pub fn finish(self) -> Result<(), FieldError> {
        if huffman_decoder().accepting[self.state] {
            Ok(())
        } else {
            Err(FieldError::Huffman)
        }
    }
}

fn huffman_decode(input: &[u8], output: &mut Vec<u8>) -> Result<(), FieldError> {
    let mut huffman = Huffman::default();
    huffman.feed(input, output)?;
    huffman.finish()
}

// String literals: an H flag, a length with a prefix of `prefix` bits, then
// the octets, Huffman-coded when that is shorter.

pub fn encode_string(value: &[u8], prefix: u8, flags: u8, output: &mut Vec<u8>) {
    let coded = huffman_length(value);
    if coded < value.len() {
        encode_integer(coded, prefix, flags | (1 << prefix), output);
        huffman_encode(value, output);
    } else {
        encode_integer(value.len(), prefix, flags, output);
        output.extend_from_slice(value);
    }
}

pub fn decode_string(
    input: &[u8],
    position: &mut usize,
    prefix: u8,
    output: &mut Vec<u8>,
) -> Result<(), FieldError> {
    let huffman = *input.get(*position).ok_or(FieldError::Truncated)? & (1 << prefix) != 0;
    let length = decode_integer(input, position, prefix)?;
    let bytes = input
        .get(*position..*position + length)
        .ok_or(FieldError::Truncated)?;
    *position += length;
    if huffman {
        huffman_decode(bytes, output)
    } else {
        output.extend_from_slice(bytes);
        Ok(())
    }
}

// Decoded fields.

/// The fields of a decoded section, stored in one reusable buffer.
#[derive(Default)]
pub struct Fields {
    bytes: Vec<u8>,
    /// End offsets of each name and value in `bytes`.
    ends: Vec<(usize, usize)>,
}
impl Fields {
    fn clear(&mut self) {
        self.bytes.clear();
        self.ends.clear();
    }
    fn push_static(&mut self, (name, value): (&str, &str)) {
        self.bytes.extend_from_slice(name.as_bytes());
        let name_end = self.bytes.len();
        self.bytes.extend_from_slice(value.as_bytes());
        self.ends.push((name_end, self.bytes.len()));
    }
    fn end_name(&mut self) -> usize {
        self.bytes.len()
    }
    fn end_value(&mut self, name_end: usize) {
        self.ends.push((name_end, self.bytes.len()));
    }
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        let mut start = 0;
        self.ends.iter().map(move |&(name_end, value_end)| {
            let field = (
                &self.bytes[start..name_end],
                &self.bytes[name_end..value_end],
            );
            start = value_end;
            field
        })
    }
    /// See [`fields_checksum`].
    pub fn checksum(&self) -> u64 {
        fields_checksum(self.iter())
    }
}

/// The checksum the workload expects of a field section.
pub fn fields_checksum<'a>(fields: impl Iterator<Item = (&'a [u8], &'a [u8])>) -> u64 {
    fields
        .map(|(name, value)| crate::common::byte_sum(name) + crate::common::byte_sum(value) + 7)
        .sum()
}

struct Lookup {
    exact: HashMap<(&'static str, &'static str), usize>,
    name: HashMap<&'static str, usize>,
}
fn lookup(table: &'static [(&'static str, &'static str)]) -> Lookup {
    let mut lookup = Lookup {
        exact: HashMap::new(),
        name: HashMap::new(),
    };
    for (index, &(name, value)) in table.iter().enumerate() {
        lookup.exact.entry((name, value)).or_insert(index);
        lookup.name.entry(name).or_insert(index);
    }
    lookup
}

// HPACK.

pub mod hpack {
    use super::*;

    fn table() -> &'static Lookup {
        static LOOKUP: OnceLock<Lookup> = OnceLock::new();
        LOOKUP.get_or_init(|| lookup(&HPACK_STATIC))
    }

    /// Encode a field block: indexed fields where the static table has the
    /// field, otherwise literals without indexing.
    pub fn encode<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>, output: &mut Vec<u8>) {
        let table = table();
        for (name, value) in fields {
            if let Some(&index) = table.exact.get(&(name, value)) {
                encode_integer(index + 1, 7, 0x80, output);
            } else if let Some(&index) = table.name.get(name) {
                encode_integer(index + 1, 4, 0x00, output);
                encode_string(value.as_bytes(), 7, 0, output);
            } else {
                output.push(0x00);
                encode_string(name.as_bytes(), 7, 0, output);
                encode_string(value.as_bytes(), 7, 0, output);
            }
        }
    }

    fn entry(index: usize) -> Result<(&'static str, &'static str), FieldError> {
        index
            .checked_sub(1)
            .and_then(|index| HPACK_STATIC.get(index).copied())
            .ok_or(FieldError::Unsupported)
    }

    pub fn decode(input: &[u8], fields: &mut Fields) -> Result<(), FieldError> {
        fields.clear();
        let mut position = 0;
        while position < input.len() {
            let first = input[position];
            if first & 0x80 != 0 {
                let index = decode_integer(input, &mut position, 7)?;
                fields.push_static(entry(index)?);
            } else if first & 0xe0 == 0x00 {
                // Literal without indexing (0000) or never indexed (0001).
                let index = decode_integer(input, &mut position, 4)?;
                let name_end = if index == 0 {
                    decode_string(input, &mut position, 7, &mut fields.bytes)?;
                    fields.end_name()
                } else {
                    fields.bytes.extend_from_slice(entry(index)?.0.as_bytes());
                    fields.end_name()
                };
                decode_string(input, &mut position, 7, &mut fields.bytes)?;
                fields.end_value(name_end);
            } else {
                // Incremental indexing or a table size update.
                return Err(FieldError::Unsupported);
            }
        }
        Ok(())
    }
}

// QPACK.

pub mod qpack {
    use super::*;

    fn table() -> &'static Lookup {
        static LOOKUP: OnceLock<Lookup> = OnceLock::new();
        LOOKUP.get_or_init(|| lookup(&QPACK_STATIC))
    }

    /// The static table index of this exact field, if any.
    pub fn static_field(name: &str, value: &str) -> Option<usize> {
        table().exact.get(&(name, value)).copied()
    }

    /// The static table index of a field with this name, if any.
    pub fn static_name(name: &str) -> Option<usize> {
        table().name.get(name).copied()
    }

    /// Encode a field section that refers to no dynamic table entry.
    pub fn encode<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>, output: &mut Vec<u8>) {
        let table = table();
        // Required Insert Count 0, Base 0.
        output.extend_from_slice(&[0x00, 0x00]);
        for (name, value) in fields {
            if let Some(&index) = table.exact.get(&(name, value)) {
                // Indexed field line, static table.
                encode_integer(index, 6, 0xc0, output);
            } else if let Some(&index) = table.name.get(name) {
                // Literal field line with name reference, static table.
                encode_integer(index, 4, 0x50, output);
                encode_string(value.as_bytes(), 7, 0, output);
            } else {
                // Literal field line with literal name.
                encode_string(name.as_bytes(), 3, 0x20, output);
                encode_string(value.as_bytes(), 7, 0, output);
            }
        }
    }

    pub fn entry(index: usize) -> Result<(&'static str, &'static str), FieldError> {
        QPACK_STATIC.get(index).copied().ok_or(FieldError::Index)
    }

    pub fn decode(input: &[u8], fields: &mut Fields) -> Result<(), FieldError> {
        fields.clear();
        let mut position = 0;
        if decode_integer(input, &mut position, 8)? != 0 {
            return Err(FieldError::Unsupported);
        }
        decode_integer(input, &mut position, 7)?;
        while position < input.len() {
            let first = input[position];
            if first & 0x80 != 0 {
                if first & 0x40 == 0 {
                    return Err(FieldError::Unsupported);
                }
                let index = decode_integer(input, &mut position, 6)?;
                fields.push_static(entry(index)?);
            } else if first & 0x40 != 0 {
                if first & 0x10 == 0 {
                    return Err(FieldError::Unsupported);
                }
                let index = decode_integer(input, &mut position, 4)?;
                fields.bytes.extend_from_slice(entry(index)?.0.as_bytes());
                let name_end = fields.end_name();
                decode_string(input, &mut position, 7, &mut fields.bytes)?;
                fields.end_value(name_end);
            } else if first & 0x20 != 0 {
                decode_string(input, &mut position, 3, &mut fields.bytes)?;
                let name_end = fields.end_name();
                decode_string(input, &mut position, 7, &mut fields.bytes)?;
                fields.end_value(name_end);
            } else {
                // Post-base references need a dynamic table.
                return Err(FieldError::Unsupported);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let digits: String = text.split_whitespace().collect();
        (0..digits.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap())
            .collect()
    }

    fn decoded(fields: &Fields) -> Vec<(String, String)> {
        fields
            .iter()
            .map(|(n, v)| {
                (
                    String::from_utf8(n.to_vec()).unwrap(),
                    String::from_utf8(v.to_vec()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn huffman_matches_rfc_7541_examples() {
        // Appendix C.4.1 and C.4.3.
        for (text, coded) in [
            ("www.example.com", "f1e3 c2e5 f23a 6ba0 ab90 f4ff"),
            ("custom-key", "25a8 49e9 5ba9 7d7f"),
            ("custom-value", "25a8 49e9 5bb8 e8b4 bf"),
        ] {
            let mut encoded = Vec::new();
            huffman_encode(text.as_bytes(), &mut encoded);
            assert_eq!(encoded, hex(coded));
            let mut decoded = Vec::new();
            huffman_decode(&encoded, &mut decoded).unwrap();
            assert_eq!(decoded, text.as_bytes());
        }
    }

    #[test]
    fn huffman_encodes_the_same_a_piece_at_a_time() {
        let input: Vec<u8> = (0..=255)
            .chain(b"no-cache, private".iter().copied())
            .collect();
        let mut whole = Vec::new();
        huffman_encode(&input, &mut whole);
        for room in [1, 3, 64] {
            let (mut encoder, mut pieces, mut piece) =
                (HuffmanEncoder::new(&input), Vec::new(), vec![0; room]);
            loop {
                let filled = encoder.fill(&mut piece);
                if filled == 0 {
                    break;
                }
                pieces.extend_from_slice(&piece[..filled]);
            }
            assert_eq!(pieces, whole, "{room}");
        }
    }

    #[test]
    fn huffman_round_trips_every_byte_and_rejects_bad_padding() {
        let bytes: Vec<u8> = (0..=255).collect();
        let mut encoded = Vec::new();
        huffman_encode(&bytes, &mut encoded);
        let mut decoded = Vec::new();
        huffman_decode(&encoded, &mut decoded).unwrap();
        assert_eq!(decoded, bytes);
        // "0" is 00000 (5 bits); padding with zeros is not a prefix of EOS.
        assert_eq!(
            huffman_decode(&[0x00], &mut Vec::new()),
            Err(FieldError::Huffman)
        );
        // Eight bits of padding are too many.
        assert_eq!(
            huffman_decode(&[0xff], &mut Vec::new()),
            Err(FieldError::Huffman)
        );
    }

    #[test]
    fn hpack_decodes_rfc_7541_examples() {
        let mut fields = Fields::default();
        // Appendix C.2.2: literal without indexing, indexed name.
        hpack::decode(&hex("040c 2f73 616d 706c 652f 7061 7468"), &mut fields).unwrap();
        assert_eq!(decoded(&fields), [(":path".into(), "/sample/path".into())]);
        // Appendix C.2.4: indexed.
        hpack::decode(&hex("82"), &mut fields).unwrap();
        assert_eq!(decoded(&fields), [(":method".into(), "GET".into())]);
    }

    #[test]
    fn qpack_decodes_rfc_9204_example() {
        // Appendix B.1.
        let mut fields = Fields::default();
        qpack::decode(&hex("0000 510b 2f69 6e64 6578 2e68 746d 6c"), &mut fields).unwrap();
        assert_eq!(decoded(&fields), [(":path".into(), "/index.html".into())]);
    }

    #[test]
    fn both_round_trip_static_name_and_literal_fields() {
        let input = [
            (":status", "200"),
            ("content-type", "image/png"),
            ("x-fb-debug", "SASLTVtJVp+AQ2p1v8FGiCfzyMTFyXtCl4jQ=="),
            ("content-length", "123456"),
        ];
        for (encode, decode) in [
            (
                hpack::encode as fn(_, &mut Vec<u8>),
                hpack::decode as fn(&[u8], &mut Fields) -> _,
            ),
            (qpack::encode, qpack::decode),
        ] {
            let mut encoded = Vec::new();
            encode(input.iter().copied(), &mut encoded);
            let mut fields = Fields::default();
            decode(&encoded, &mut fields).unwrap();
            let expected: Vec<(String, String)> =
                input.iter().map(|&(n, v)| (n.into(), v.into())).collect();
            assert_eq!(decoded(&fields), expected);
        }
    }
}
