// expect: pass
#![feature(coroutine_trait)]
use sakuya_data_frame_codec_example::current_codec::{ReadVarintExt,CodecError};
use std::pin::Pin;
async fn read<T:ReadVarintExt+?Sized>(source:Pin<&mut T>)->Result<u64,CodecError>{source.read_varint().await}
fn main(){}
