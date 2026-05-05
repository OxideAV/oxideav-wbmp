//! Standalone image container returned by `oxideav-wbmp`'s framework-free
//! decode API and accepted by the standalone encode API.
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate can be built with the default `registry` feature off — i.e.
//! without depending on `oxideav-core` at all. When the `registry`
//! feature is on the [`crate::registry`] module exposes the
//! [`WbmpPixelFormat`] ↔ `oxideav_core::PixelFormat` mapping plus the
//! `From<WbmpError> for oxideav_core::Error` impl so the trait-side
//! `Decoder` / `Encoder` impls keep working unchanged.

/// Pixel layout used by [`WbmpImage`]. WBMP Type 0 only carries
/// monochrome 1-bit-per-pixel data, so the enum has a single variant
/// today; it stays here as an enum for symmetry with the rest of the
/// workspace's image crates and so future WBMP types (greyscale,
/// colour) — should they ever ship — can land as additional variants
/// without a breaking API change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbmpPixelFormat {
    /// 1 bit per pixel, MSB-first packed, **1 = white, 0 = black**
    /// per WAP-237 §8.4. Rows are padded to a byte boundary —
    /// `stride = ceil(width / 8)`. Maps to `oxideav_core::PixelFormat::MonoWhite`
    /// when the `registry` feature is on.
    MonoWhite,
}

/// One image plane: row-major bytes plus the row stride in bytes.
///
/// Mirrors `oxideav_core::VideoPlane` so the registry-side conversion
/// is a trivial field-by-field copy.
#[derive(Debug, Clone)]
pub struct WbmpPlane {
    /// Bytes per row in `data` — for [`WbmpPixelFormat::MonoWhite`]
    /// this is `ceil(width / 8)`.
    pub stride: usize,
    /// Raw plane bytes, packed `stride × height`. Bits within each
    /// byte are MSB-first; bit `1` = white, bit `0` = black; trailing
    /// bits in the last byte of every row are zero-padded by the
    /// encoder (and ignored by the decoder).
    pub data: Vec<u8>,
}

/// One decoded WBMP frame, framework-free shape.
///
/// `pts` is `None` for the standalone [`crate::parse_wbmp`] entry
/// point — that function operates on a single isolated file buffer
/// without packet timing information. The registry-backed `Decoder`
/// impl still passes `pts` through from the surrounding `Packet`.
#[derive(Debug, Clone)]
pub struct WbmpImage {
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Pixel layout the planes carry. Always
    /// [`WbmpPixelFormat::MonoWhite`] in this round.
    pub pixel_format: WbmpPixelFormat,
    /// One [`WbmpPlane`] per plane. WBMP only ever ships a single
    /// packed plane, so this is always `len() == 1`.
    pub planes: Vec<WbmpPlane>,
    /// Optional presentation timestamp. Always `None` from the
    /// standalone decode path.
    pub pts: Option<i64>,
}

impl WbmpImage {
    /// `ceil(width / 8)` — number of bytes a single row occupies in
    /// the packed plane.
    pub fn row_stride(width: u32) -> usize {
        (width as usize).div_ceil(8)
    }
}
