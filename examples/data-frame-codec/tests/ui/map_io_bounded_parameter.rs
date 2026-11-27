// expect: the IO parameter is replaced by any type, so it takes no bounds
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
#[map_io(S)]
struct BoundedParameter<S>(S)
where
    S: Clone;

fn main() {}
