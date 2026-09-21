// expect: pass
#![feature(coroutine_trait)]
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_data_frame_codec_example::current_codec::{ReadVarintExt,ReadSource,CodecError};
use std::pin::Pin;
async fn read<F:ReadSource,T:ReadVarintExt<F>+?Sized>(source:Pin<&mut T>)->Result<u64,CodecError>{source.read_varint().await}
fn main(){}
