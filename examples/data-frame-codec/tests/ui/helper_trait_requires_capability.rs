// expect: the trait bound `(): ReadVarintExt` is not satisfied
use nitori_data_frame_codec_example::current_codec::ReadVarintExt;
fn supports<T:ReadVarintExt>(){}
fn main(){supports::<()>();}
