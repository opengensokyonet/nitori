// expect: expected `within = P` or a type parameter
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
#[map_io(other = S)]
struct WrongAttribute<S>(S);

fn main() {}
