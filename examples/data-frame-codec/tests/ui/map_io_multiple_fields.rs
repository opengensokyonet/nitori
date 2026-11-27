// expect: the IO parameter must appear in exactly one field of each struct or variant
// facade-only
use nitori_codec::staged::MapIo;

#[derive(MapIo)]
#[map_io(S)]
enum MultipleFields<S> {
    One(S),
    Two(S, S),
}

fn main() {}
