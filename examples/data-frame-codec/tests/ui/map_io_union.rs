// expect: MapIo is derived for structs and enums
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
#[map_io(S)]
union Unsupported<S> {
    io: core::mem::ManuallyDrop<S>,
}

fn main() {}
