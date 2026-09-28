//! Compose partial writes and report progress before completing a line.
#![feature(coroutines, coroutine_trait, type_alias_impl_trait)]

use nitori_call::{Target, call};
use nitori_io::{TargetWriteExt, Write, calls::WriteAllError};
use std::{ops::CoroutineState, pin::pin};

#[call(sync, yields = usize)]
async fn write_line<'data, H: Write>(
    io: Target<H>,
    line: &'data [u8],
) -> Result<usize, WriteAllError<H::Error>> {
    let written = io.write_all(line).await.result?;
    yield written;
    io.write_all(b"\n".as_slice()).await.result?;
    Ok(written + 1)
}

fn main() {
    let mut output = Vec::new();
    {
        let mut events = pin!(output.sync_write_line_unpin(b"hello"));
        for event in events.as_mut() {
            match event {
                CoroutineState::Yielded(written) => println!("payload written: {written}"),
                CoroutineState::Complete(result) => println!("line written: {}", result.unwrap()),
            }
        }
    }
    assert_eq!(output, b"hello\n");
}
