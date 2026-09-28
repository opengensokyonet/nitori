// expect: pass
#![feature(coroutine_trait)]
use nitori_call::{TargetExt as _, PollCallExt as _};
use nitori_data_frame_codec_example::current_codec::ReadVarintExt;
use std::pin::Pin;
async fn read<F:Read,T:ReadVarintExt<F>+?Sized>(source:Pin<&mut T>)->Result<u64,ReadBeError<F::Error>>{source.read_varint().await}
use nitori_io::{Read, calls::ReadBeError};
fn main(){}
