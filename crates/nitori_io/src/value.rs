//! A value with the metadata or write options of its bytes.

/// A value with the attributes of its bytes.
///
/// Read, as [`read_array`](crate::calls::read_array),
/// [`read_le`](crate::calls::read_le) and [`read_be`](crate::calls::read_be)
/// return it, `metadata` is the combined [`Metadata`](crate::Read::Metadata)
/// of the bytes the value was decoded from. Written, `metadata` is the write
/// options of the value's bytes, the `O` of [`Write<O>`](crate::Write).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Value<V, M> {
    /// The value.
    pub value: V,
    /// The metadata of its bytes when read, or their write options when
    /// written.
    pub metadata: M,
}
