//! Lossless conversions of encoded lengths.
use nitori_codec::Widen;

macro_rules! assert_widens {
    ($($from:ty => $($to:ty),+;)+) => {$($(
        let widened = <$from as Widen<$to>>::widen(<$from>::MAX);
        assert_eq!(widened as u128, <$from>::MAX as u128);
    )+)+};
}

#[test]
fn unsigned_lengths_widen_without_losing_bits() {
    assert_widens! {
        u8 => u8, u16, u32, u64, u128, usize;
        u16 => u16, u32, u64, u128, usize;
        u32 => u32, u64, u128;
        u64 => u64, u128;
        u128 => u128;
        usize => usize, u128;
    }
}

#[test]
fn pointer_sized_lengths_widen_according_to_pointer_width() {
    #[cfg(target_pointer_width = "16")]
    assert_widens! {
        usize => u32, u64;
    }
    #[cfg(target_pointer_width = "32")]
    assert_widens! {
        u32 => usize;
        usize => u64;
    }
    #[cfg(target_pointer_width = "64")]
    assert_widens! {
        u32 => usize;
        u64 => usize;
        usize => u64;
    }
}
