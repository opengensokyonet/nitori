// expect: Direct<()>: ReadSource
use nitori_call::{ReceiverExt as _, PollCallExt as _};
use nitori_data_frame_codec_example::current_codec::ReadVarintExt;
fn supports<T:ReadVarintExt<nitori_call::Direct<()>>>(){}
fn main(){supports::<()>();}
