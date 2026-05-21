//! Crate-local error type used by `oxideav-wbmp`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`WbmpError`] gains a
//! `From<WbmpError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! underlying parse/encode functions stay framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-wbmp`. Standalone (no `oxideav-core`)
/// callers see this; framework callers convert via the gated
/// `From<WbmpError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, WbmpError>;

/// Error variants returned by `oxideav-wbmp`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit. The crate intentionally avoids surfacing transport (`Io`)
/// or framework-specific (`FormatNotFound`, `CodecNotFound`) errors —
/// those originate in callers that are already linking `oxideav-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WbmpError {
    /// The byte stream is malformed (truncated header, MBI overflows
    /// the 32-bit value range, declared image body shorter than what
    /// `width * height` requires, …).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement —
    /// in practice, any non-zero Type field. WAP-237 only standardises
    /// Type 0; no other type is widely deployed.
    Unsupported(String),
    /// The byte stream declares dimensions or a pixel-data size that
    /// exceeds the caller-configured [`crate::WbmpLimits`]. Raised
    /// before the decoder allocates the pixel buffer so the host stays
    /// safe even against a malicious 1 GB-bitmap header.
    LimitExceeded(String),
}

impl WbmpError {
    /// Construct a [`WbmpError::InvalidData`] from a stringy message.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct a [`WbmpError::Unsupported`] from a stringy message.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    /// Construct a [`WbmpError::LimitExceeded`] from a stringy message.
    pub fn limit_exceeded(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl fmt::Display for WbmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
        }
    }
}

impl std::error::Error for WbmpError {}
