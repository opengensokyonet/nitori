use snafu::Snafu;

/// A source of cryptographically secure random bytes.
pub trait Random {
    /// `N` random bytes.
    ///
    /// # Errors
    ///
    /// Fails when the source cannot produce them, for example when the
    /// operating system's generator reports an error.
    fn random<const N: usize>(&self) -> Result<[u8; N], RandomError>;
}

/// The random source failed to produce bytes.
#[derive(Debug, Snafu)]
#[snafu(display("random source failed"))]
pub struct RandomError;
