// expect: name the IO: `#[map_io(S)]` or `#[map_io(within = P)]`
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
struct MissingAttribute<S>(S);

fn main() {}
