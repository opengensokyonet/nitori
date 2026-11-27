//! Replacing IO in structs, enums, and nested values.
use nitori_codec::staged::{MapIo, Vacant, take_io};

#[derive(Debug, PartialEq, Eq, MapIo)]
#[map_io(S)]
struct Record<'a, S, const N: usize> {
    name: &'a str,
    io: S,
    bytes: [u8; N],
}

#[derive(Debug, PartialEq, Eq, MapIo)]
#[map_io(S)]
enum Event<S> {
    Header { source: S, kind: u8 },
    Payload(Record<'static, S, 2>, usize),
}

#[derive(Debug, PartialEq, Eq, MapIo)]
#[map_io(within = P)]
struct Envelope<P>(u16, P);

#[test]
fn extracting_io_keeps_the_other_fields() {
    let record = Record {
        name: "data",
        io: vec![1, 2],
        bytes: *b"ab",
    };
    let (vacant, io): (Record<'_, Vacant, 2>, _) = take_io(record);
    assert_eq!(io, [1, 2]);
    let record = vacant.map_io(|_| "replacement");
    assert_eq!(
        record,
        Record {
            name: "data",
            io: "replacement",
            bytes: *b"ab"
        }
    );
}

#[test]
fn mapping_preserves_the_enum_variant_and_nested_fields() {
    let event = Event::Header { source: 4, kind: 9 };
    assert_eq!(
        event.map_io(|io| io.to_string()),
        Event::Header {
            source: "4".to_owned(),
            kind: 9
        }
    );

    let event = Envelope(
        7,
        Event::Payload(
            Record {
                name: "data",
                io: &b"xy"[..],
                bytes: *b"ab",
            },
            3,
        ),
    );
    let (vacant, io): (Envelope<Event<Vacant>>, _) = take_io(event);
    assert_eq!(io, b"xy");
    let mapped = vacant.map_io(|_| vec![1, 2]);
    assert_eq!(
        mapped,
        Envelope(
            7,
            Event::Payload(
                Record {
                    name: "data",
                    io: vec![1, 2],
                    bytes: *b"ab"
                },
                3
            )
        )
    );
}
