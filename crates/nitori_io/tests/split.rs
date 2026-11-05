use bytes::{Bytes, BytesMut};
use nitori_io::SplitTo;

fn pieces<P: SplitTo + AsRef<[u8]>>(mut buffer: P, lengths: &[usize]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut out = Vec::new();
    for &length in lengths {
        out.push(buffer.split_to(length).unwrap().as_ref().to_vec());
    }
    (out, buffer.as_ref().to_vec())
}

#[test]
fn slice_pieces_are_consecutive_and_borrowed() {
    let data = b"abcdefgh";
    let (out, rest) = pieces(data.as_slice(), &[2, 0, 3]);
    assert_eq!(out, [b"ab".to_vec(), Vec::new(), b"cde".to_vec()]);
    assert_eq!(rest, b"fgh");

    let mut cursor = data.as_slice();
    let head = cursor.split_to(4).unwrap();
    assert_eq!(head.as_ptr(), data.as_ptr());
    assert_eq!(cursor.as_ptr(), data[4..].as_ptr());
}

#[test]
fn bytes_mut_pieces_share_the_allocation_and_write_independently() {
    let mut buffer = BytesMut::from(&b"abcdefgh"[..]);
    let start = buffer.as_ptr();
    let mut head = SplitTo::split_to(&mut buffer, 3).unwrap();
    assert_eq!(head.as_ptr(), start);
    assert_eq!(buffer.as_ptr(), start.wrapping_add(3));
    head[0] = b'X';
    buffer[0] = b'Y';
    assert_eq!(&head[..], b"Xbc");
    assert_eq!(&buffer[..], b"Yefgh");
}

#[test]
fn splitting_everything_leaves_an_empty_rest() {
    let (out, rest) = pieces(BytesMut::from(&b"abc"[..]), &[3]);
    assert_eq!(out, [b"abc".to_vec()]);
    assert!(rest.is_empty());
    let (out, rest) = pieces(b"abc".as_slice(), &[3]);
    assert_eq!(out, [b"abc".to_vec()]);
    assert!(rest.is_empty());
}

#[test]
fn bytes_pieces_share_the_allocation() {
    let mut buffer = Bytes::from_static(b"abcdefgh");
    let start = buffer.as_ptr();
    let head = SplitTo::split_to(&mut buffer, 3).unwrap();
    assert_eq!(head.as_ptr(), start);
    assert_eq!(buffer.as_ptr(), start.wrapping_add(3));
    assert_eq!(&head[..], b"abc");
    assert_eq!(&buffer[..], b"defgh");

    let (out, rest) = pieces(Bytes::from_static(b"abcdefgh"), &[2, 0, 3]);
    assert_eq!(out, [b"ab".to_vec(), Vec::new(), b"cde".to_vec()]);
    assert_eq!(rest, b"fgh");
}

#[test]
fn slice_split_beyond_the_length_fails_and_keeps_the_buffer() {
    let mut buffer = b"ab".as_slice();
    let error = buffer.split_to(3).unwrap_err();
    assert_eq!((error.at(), error.length()), (3, 2));
    assert_eq!(buffer, b"ab");
    assert_eq!(buffer.split_to(2).unwrap(), b"ab");
}

#[test]
fn bytes_mut_split_beyond_the_length_fails_and_keeps_the_buffer() {
    let mut buffer = BytesMut::from(&b"ab"[..]);
    let error = SplitTo::split_to(&mut buffer, 3).unwrap_err();
    assert_eq!((error.at(), error.length()), (3, 2));
    assert_eq!(&buffer[..], b"ab");
}

#[test]
fn bytes_split_beyond_the_length_fails_and_keeps_the_buffer() {
    let mut buffer = Bytes::from_static(b"ab");
    let error = SplitTo::split_to(&mut buffer, 3).unwrap_err();
    assert_eq!((error.at(), error.length()), (3, 2));
    assert_eq!(
        error.to_string(),
        "cannot cut 3 bytes off a buffer of 2 bytes"
    );
    assert_eq!(&buffer[..], b"ab");
}
