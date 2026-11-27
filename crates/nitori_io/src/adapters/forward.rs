// Generate a family's capability impl from its inner view. `pass` forwards
// the call unchanged. `with` lets the wrapper adjust the call via a helper.
macro_rules! forward {
    (@call pass; $host:ident, $cx:ident, ($($arg:expr),*), $closure:expr) => {
        ($closure)($host.io_pin(), $cx, $($arg),*)
    };
    (@call with $h:ident; $host:ident, $cx:ident, ($($arg:expr),*), $closure:expr) => {
        $host.$h($cx, $($arg,)* $closure)
    };

    (Read for [$($g:tt)*] $fam:ty, inner $F:ty , Metadata = $meta:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::Read for $fam $(where $($w)*)? {
            type Error = <$F as $crate::Read>::Error;
            type Metadata = $meta;
            fn poll_read<'v, Output: ::bytes::BufMut + ?Sized>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
                output: &mut Output,
            ) -> ::core::task::Poll<$crate::ReadResult<Self>>
            where
                Self: 'v,
            {
                forward!(@call $how $($h)?; host, cx, (output),
                    |io, cx, output| <$F as $crate::Read>::poll_read(io, cx, output))
            }
        }
    };
    (ReadChunk for [$($g:tt)*] $fam:ty, inner $F:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::ReadChunk for $fam $(where $($w)*)? {
            type Chunk = <$F as $crate::ReadChunk>::Chunk;
            fn poll_read_chunk<'v>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
                maximum: ::core::num::NonZeroUsize,
            ) -> ::core::task::Poll<$crate::ChunkResult<Self>>
            where
                Self: 'v,
            {
                forward!(@call $how $($h)?; host, cx, (maximum),
                    |io, cx, maximum| <$F as $crate::ReadChunk>::poll_read_chunk(io, cx, maximum))
            }
        }
    };
    (Skip for [$($g:tt)*] $fam:ty, inner $F:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::Skip for $fam $(where $($w)*)? {
            type Count = <$F as $crate::Skip>::Count;
            fn poll_skip<'v, Maximum>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
                maximum: Maximum,
            ) -> ::core::task::Poll<Result<Option<Self::Count>, <Self as $crate::Read>::Error>>
            where
                Self: 'v,
                Maximum: $crate::Min<Self::Count, Output = Self::Count> + Clone,
            {
                forward!(@call $how $($h)?; host, cx, (maximum),
                    |io, cx, maximum| <$F as $crate::Skip>::poll_skip(io, cx, maximum))
            }
        }
    };
    (ExactLen for [$($g:tt)*] $fam:ty, inner $F:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::ExactLen for $fam $(where $($w)*)? {
            type Length = <$F as $crate::ExactLen>::Length;
            type LengthError = <$F as $crate::ExactLen>::LengthError;
            fn poll_remaining_len<'v>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<Result<Self::Length, Self::LengthError>>
            where
                Self: 'v,
            {
                forward!(@call $how $($h)?; host, cx, (),
                    |io, cx| <$F as $crate::ExactLen>::poll_remaining_len(io, cx))
            }
        }
    };
    (Write<$O:ty> for [$($g:tt)*] $fam:ty, inner $F:ty, options $IO:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::Write<$O> for $fam $(where $($w)*)? {
            type Error = <$F as $crate::Write<$IO>>::Error;
            type Report = <$F as $crate::Write<$IO>>::Report;
            fn poll_write<'v, I: ::bytes::Buf + ?Sized>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
                input: &mut I,
                options: &$O,
            ) -> ::core::task::Poll<$crate::WriteResult<Self, $O>>
            where
                Self: 'v,
            {
                forward!(@call $how $($h)?; host, cx, (input, options),
                    |io, cx, input, options| <$F as $crate::Write<$IO>>::poll_write(io, cx, input, options))
            }
        }
    };
    // Only for a family that lends the inner storage with the same options:
    // the fill goes to the inner sink as it is.
    (WriteLent<$O:ty> for [$($g:tt)*] $fam:ty, inner $F:ty, $how:ident $($h:ident)? $(, where [$($w:tt)*])?) => {
        impl<$($g)*> $crate::WriteLent<$O> for $fam $(where $($w)*)? {
            type Buffer<'l> = <$F as $crate::WriteLent<$O>>::Buffer<'l>;
            fn poll_write_lent<'v, T, A, Fill>(
                host: ::core::pin::Pin<&mut Self::ReceiverView<'v>>,
                cx: &mut ::core::task::Context<'_>,
                wanted: ::core::num::NonZeroUsize,
                fill: Fill,
            ) -> ::core::task::Poll<$crate::WriteLentResult<Self, $O, T, A>>
            where
                Self: 'v,
                Fill: for<'l> FnOnce(&mut $crate::LentBuffer<Self::Buffer<'l>>) -> Result<$crate::Filled<T, $O>, A>,
            {
                forward!(@call $how $($h)?; host, cx, (wanted),
                    |io, cx, wanted| <$F as $crate::WriteLent<$O>>::poll_write_lent(io, cx, wanted, fill))
            }
        }
    };
}
