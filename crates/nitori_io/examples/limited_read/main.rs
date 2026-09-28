//! A length-prefixed record cannot let its child parser consume the next record.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

mod limit;

use limit::ReadLimit;
use nitori_call::{Target, TargetExt, call};
use nitori_io::{Read, TargetReadExt, calls::ReadArrayError};

// This parser can be reused with either an unrestricted or a limited receiver.
#[call(sync)]
async fn read_word<H: Read>(io: Target<H>) -> Result<[u8; 2], ReadArrayError<H::Error>> {
    io.read_array::<2>().await
}

#[call(sync)]
async fn read_record<H: Read>(io: Target<H>) -> Result<[u8; 2], ReadArrayError<H::Error>> {
    let [length] = io.read_array::<1>().await?;
    io.compose(ReadLimit(usize::from(length))).read_word().await
}

fn main() {
    // Without the boundary, a two-byte parser consumes the next record's length.
    let mut payload = b"A\x02BC".as_slice();
    assert_eq!(payload.sync_read_word_unpin().unwrap(), *b"A\x02");

    // The malformed first record has only one byte; the next record stays readable.
    let mut input = b"\x01A\x02BC".as_slice();
    let error = input.sync_read_record_unpin().unwrap_err();
    assert_eq!(error.completed(), 1);
    assert_eq!(input, b"\x02BC");
    assert_eq!(input.sync_read_record_unpin().unwrap(), *b"BC");
}

#[cfg(test)]
mod tests;
