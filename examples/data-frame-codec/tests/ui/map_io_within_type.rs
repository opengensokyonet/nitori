// expect: with `within = P`, the field's type is `P`
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
#[map_io(within = P)]
struct WithinType<P>(Option<P>);

fn main() {}
