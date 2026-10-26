// expect: nitori_io::Read
// A resource whose family lacks the capability has no generated method.
use nitori_data_frame_codec_example::current_codec::ReadVarintHostExt as _;
fn main() { let _ = ().read_varint_unpin(); }
