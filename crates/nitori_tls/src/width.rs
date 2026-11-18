//! The widths of length fields.

/// The width of a length field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    /// One byte.
    U8,
    /// Two bytes.
    U16,
    /// Three bytes.
    U24,
}

impl Width {
    /// The number of bytes of the field.
    pub const fn len(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
            Self::U24 => 3,
        }
    }

    /// The largest length the field holds.
    pub const fn max(self) -> usize {
        match self {
            Self::U8 => 0xff,
            Self::U16 => 0xffff,
            Self::U24 => 0xff_ffff,
        }
    }
}
