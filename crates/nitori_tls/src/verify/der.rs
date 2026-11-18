//! A minimal DER reader for the parts of X.509 the verifier reads itself:
//! the SubjectPublicKeyInfo of a key, and the validity and extensions of a
//! certificate.
//!
//! It reads definite-length elements with one-byte tags, which is all DER
//! that X.509 uses, and keeps every element's exact bytes, so that a
//! structure can be rebuilt from its parts unchanged.

use snafu::Snafu;
use time::UtcDateTime;

/// DER that does not have the expected structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("malformed DER"))]
pub struct MalformedDerError;

pub(crate) const SEQUENCE: u8 = 0x30;
pub(crate) const OBJECT_IDENTIFIER: u8 = 0x06;
pub(crate) const BIT_STRING: u8 = 0x03;
pub(crate) const OCTET_STRING: u8 = 0x04;
pub(crate) const NULL: u8 = 0x05;
pub(crate) const INTEGER: u8 = 0x02;
pub(crate) const BOOLEAN: u8 = 0x01;
pub(crate) const UTC_TIME: u8 = 0x17;
pub(crate) const GENERALIZED_TIME: u8 = 0x18;

/// One element: its tag, its contents, and its whole encoding.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Element<'a> {
    pub(crate) tag: u8,
    pub(crate) contents: &'a [u8],
    pub(crate) encoded: &'a [u8],
}

/// Reads elements one after another from a slice.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        self.rest
    }

    pub(crate) fn peek_tag(&self) -> Option<u8> {
        self.rest.first().copied()
    }

    pub(crate) fn read(&mut self) -> Result<Element<'a>, MalformedDerError> {
        let bytes = self.rest;
        let (&tag, after_tag) = bytes.split_first().ok_or(MalformedDerError)?;
        // High tag numbers do not occur in X.509.
        if tag & 0x1f == 0x1f {
            return Err(MalformedDerError);
        }
        let (&first, mut after_len) = after_tag.split_first().ok_or(MalformedDerError)?;
        let len = match first {
            0..=0x7f => usize::from(first),
            0x81..=0x83 => {
                let count = usize::from(first & 0x7f);
                if after_len.len() < count {
                    return Err(MalformedDerError);
                }
                let (len_bytes, rest) = after_len.split_at(count);
                after_len = rest;
                len_bytes
                    .iter()
                    .fold(0, |len, &byte| (len << 8) | usize::from(byte))
            }
            // Indefinite lengths are not DER, and longer lengths do not fit
            // a handshake message.
            _ => return Err(MalformedDerError),
        };
        if after_len.len() < len {
            return Err(MalformedDerError);
        }
        let header = bytes.len() - after_len.len();
        let (encoded, rest) = bytes.split_at(header + len);
        self.rest = rest;
        Ok(Element {
            tag,
            contents: &encoded[header..],
            encoded,
        })
    }

    pub(crate) fn expect(&mut self, tag: u8) -> Result<Element<'a>, MalformedDerError> {
        let element = self.read()?;
        if element.tag != tag {
            return Err(MalformedDerError);
        }
        Ok(element)
    }

    /// Reads the element only if its tag is `tag`.
    pub(crate) fn optional(&mut self, tag: u8) -> Result<Option<Element<'a>>, MalformedDerError> {
        if self.peek_tag() == Some(tag) {
            self.read().map(Some)
        } else {
            Ok(None)
        }
    }

    /// The contents of the one element of `bytes`, of tag `tag`.
    pub(crate) fn single(bytes: &'a [u8], tag: u8) -> Result<&'a [u8], MalformedDerError> {
        let mut reader = Self::new(bytes);
        let element = reader.expect(tag)?;
        if !reader.is_empty() {
            return Err(MalformedDerError);
        }
        Ok(element.contents)
    }
}

/// Appends the header of an element of `tag` with `len` bytes of contents.
pub(crate) fn push_header(out: &mut Vec<u8>, tag: u8, len: usize) {
    out.push(tag);
    match len {
        0..=0x7f => out.push(len as u8),
        0x80..=0xff => out.extend_from_slice(&[0x81, len as u8]),
        0x100..=0xffff => {
            out.push(0x82);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(0x83);
            out.extend_from_slice(&(len as u32).to_be_bytes()[1..]);
        }
    }
}

/// An element of `tag` around `contents`.
pub(crate) fn wrap(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(contents.len() + 4);
    push_header(&mut out, tag, contents.len());
    out.extend_from_slice(contents);
    out
}

/// A UTCTime or GeneralizedTime in the forms X.509 allows: seconds, no
/// fraction, in UTC (RFC 5280 §4.1.2.5).
pub(crate) fn read_time(element: Element<'_>) -> Result<UtcDateTime, MalformedDerError> {
    let digits = element.contents;
    let (year, rest) = match (element.tag, digits.len()) {
        (UTC_TIME, 13) => {
            let year = number(&digits[..2])?;
            // RFC 5280 §4.1.2.5.1: two-digit years from 50 are 19xx.
            (
                if year >= 50 { 1900 + year } else { 2000 + year },
                &digits[2..],
            )
        }
        (GENERALIZED_TIME, 15) => (number(&digits[..4])?, &digits[4..]),
        _ => return Err(MalformedDerError),
    };
    if rest[10] != b'Z' {
        return Err(MalformedDerError);
    }
    let month = number(&rest[0..2])?;
    let day = number(&rest[2..4])?;
    let hour = number(&rest[4..6])?;
    let minute = number(&rest[6..8])?;
    let second = number(&rest[8..10])?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(MalformedDerError);
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    // Years before 1970 do not occur in certificates this verifier
    // accepts; they are malformed here.
    if seconds < 0 {
        return Err(MalformedDerError);
    }
    UtcDateTime::from_unix_timestamp(seconds).map_err(|_| MalformedDerError)
}

fn number(digits: &[u8]) -> Result<i64, MalformedDerError> {
    digits.iter().try_fold(0, |value, &digit| {
        if digit.is_ascii_digit() {
            Ok(value * 10 + i64::from(digit - b'0'))
        } else {
            Err(MalformedDerError)
        }
    })
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{
        Element, GENERALIZED_TIME, MalformedDerError, Reader, SEQUENCE, UTC_TIME, read_time, wrap,
    };
    use time::UtcDateTime;

    fn time(tag: u8, text: &str) -> Result<UtcDateTime, MalformedDerError> {
        let encoded = wrap(tag, text.as_bytes());
        read_time(Element {
            tag,
            contents: text.as_bytes(),
            encoded: &encoded,
        })
    }

    fn at(seconds: i64) -> UtcDateTime {
        UtcDateTime::from_unix_timestamp(seconds).unwrap()
    }

    #[test]
    fn lengths_short_and_long_round_trip() {
        for len in [0, 5, 0x7f, 0x80, 0xff, 0x100, 0x1_0000] {
            let contents = vec![0xab; len];
            let encoded = wrap(SEQUENCE, &contents);
            let mut reader = Reader::new(&encoded);
            let element = reader.expect(SEQUENCE).unwrap();
            assert_eq!(element.contents, &contents[..]);
            assert_eq!(element.encoded, &encoded[..]);
            assert!(reader.is_empty());
        }
    }

    #[test]
    fn malformed_elements_fail() {
        for bytes in [
            &[0x30][..],
            &[0x30, 0x02, 0x00],
            &[0x30, 0x80, 0x00, 0x00],
            &[0x30, 0x84, 0, 0, 0, 1, 0],
            &[0x1f, 0x01, 0x00],
            &[0x30, 0x81],
        ] {
            assert_eq!(Reader::new(bytes).read().unwrap_err(), MalformedDerError);
        }
        assert!(Reader::new(&[0x02, 0x00]).expect(SEQUENCE).is_err());
        assert!(Reader::single(&[0x30, 0x00, 0x30, 0x00], SEQUENCE).is_err());
    }

    #[test]
    fn times_read_in_both_forms() {
        assert_eq!(time(UTC_TIME, "700101000000Z").unwrap(), at(0));
        assert_eq!(
            time(UTC_TIME, "260929000000Z").unwrap(),
            time(GENERALIZED_TIME, "20260929000000Z").unwrap()
        );
        // 2027-04-14T23:59:59Z.
        assert_eq!(
            time(GENERALIZED_TIME, "20270414235959Z").unwrap(),
            at(1_807_747_199)
        );
        // 2000-02-29 exists; 2100-02-29 does not.
        assert!(time(GENERALIZED_TIME, "20000229000000Z").is_ok());
        assert!(time(GENERALIZED_TIME, "21000229000000Z").is_err());
        assert!(time(UTC_TIME, "260929000000+").is_err());
        assert!(time(UTC_TIME, "2609290000Z").is_err());
        assert!(time(UTC_TIME, "261329000000Z").is_err());
        assert!(time(GENERALIZED_TIME, "19691231235959Z").is_err());
    }
}
