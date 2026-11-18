//! The `signed_certificate_timestamp` extension (RFC 6962 §3.3).
//!
//! In ClientHello and CertificateRequest its data is empty, a request; in
//! a certificate entry of a Certificate message it is
//! [`SignedCertificateTimestampList`], the serialized SCTs for the
//! certificate, which this crate does not interpret.

use crate::{vector::LengthOutOfRangeError, width::Width};

/// A nonempty list of serialized SCTs, each of 1 to 65535 bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SignedCertificateTimestampList<C> {
    timestamps: Vec<C>,
}

impl<C: AsRef<[u8]>> SignedCertificateTimestampList<C> {
    /// The list of `timestamps`.
    ///
    /// # Errors
    ///
    /// [`LengthOutOfRangeError`] if the list is empty, a timestamp is empty, or
    /// the encoding is longer than 65533 bytes, so that the list fits the
    /// extension.
    pub fn new(timestamps: Vec<C>) -> Result<Self, LengthOutOfRangeError> {
        for timestamp in &timestamps {
            LengthOutOfRangeError::check(timestamp.as_ref().len(), 1..=Width::U16.max())?;
        }
        let len = timestamps.iter().map(|sct| 2 + sct.as_ref().len()).sum();
        LengthOutOfRangeError::check(len, 1..=Width::U16.max() - 2)?;
        Ok(Self { timestamps })
    }

    /// The number of bytes [`codec::write_signed_certificate_timestamp_list`]
    /// writes.
    pub fn encoded_len(&self) -> usize {
        2 + self
            .timestamps
            .iter()
            .map(|sct| 2 + sct.as_ref().len())
            .sum::<usize>()
    }
}

impl<C> SignedCertificateTimestampList<C> {
    /// The serialized SCTs.
    pub fn timestamps(&self) -> &[C] {
        &self.timestamps
    }
}

pub mod codec {
    //! Reading and writing SCT lists.

    use super::SignedCertificateTimestampList;
    use crate::{
        alert::AlertDescription,
        vector::{ReadBoundedVectorError, read_bounded_vector, write_length, write_vector},
        width::Width,
    };
    use bytes::{Buf, BufMut};
    use nitori_io::SplitTo;
    use snafu::{ResultExt, Snafu};

    /// Reads the SCT list of a certificate entry.
    ///
    /// # Errors
    ///
    /// [`ReadSignedCertificateTimestampListError`].
    pub fn read_signed_certificate_timestamp_list<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<SignedCertificateTimestampList<S>, ReadSignedCertificateTimestampListError> {
        use read_signed_certificate_timestamp_list_error::*;
        let range = 1..=Width::U16.max();
        let mut list = read_bounded_vector(src, Width::U16, range.clone()).context(ListSnafu)?;
        let mut timestamps = Vec::new();
        while list.has_remaining() {
            timestamps.push(read_serialized_sct(&mut list)?);
        }
        Ok(SignedCertificateTimestampList { timestamps })
    }

    fn read_serialized_sct<S: Buf + SplitTo>(
        src: &mut S,
    ) -> Result<S, ReadSignedCertificateTimestampListError> {
        read_bounded_vector(src, Width::U16, 1..=Width::U16.max())
            .context(read_signed_certificate_timestamp_list_error::TimestampSnafu)
    }

    /// Failures that [`read_signed_certificate_timestamp_list`] can produce.
    #[derive(Debug, Snafu)]
    #[snafu(
        module(read_signed_certificate_timestamp_list_error),
        visibility(pub(crate))
    )]
    pub enum ReadSignedCertificateTimestampListError {
        /// The list could not be read.
        #[snafu(display("SCT list read failed"))]
        List {
            /// The failure of reading the list.
            source: ReadBoundedVectorError,
        },
        /// A serialized SCT could not be read.
        #[snafu(display("serialized SCT read failed"))]
        Timestamp {
            /// The failure of reading the SCT.
            source: ReadBoundedVectorError,
        },
    }

    impl ReadSignedCertificateTimestampListError {
        /// The alert the failure calls for: always decode_error.
        pub fn alert(&self) -> AlertDescription {
            AlertDescription::DECODE_ERROR
        }
    }

    /// Writes the SCT list of a certificate entry.
    ///
    /// # Panics
    ///
    /// If `dst` has no room for what it writes, as [`BufMut`] does.
    pub fn write_signed_certificate_timestamp_list<C: AsRef<[u8]>>(
        dst: &mut impl BufMut,
        list: &SignedCertificateTimestampList<C>,
    ) {
        list.encode(dst)
    }

    impl<C: AsRef<[u8]>> SignedCertificateTimestampList<C> {
        pub(crate) fn encode(&self, dst: &mut impl BufMut) {
            write_length(dst, Width::U16, self.encoded_len() - 2);
            for timestamp in &self.timestamps {
                write_vector(dst, Width::U16, timestamp.as_ref());
            }
        }
    }
}
