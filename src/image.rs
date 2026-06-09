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

/// Pixel layout used by [`WbmpImage`]. WBMP Type 0 carries
/// monochrome 1-bit-per-pixel data; the on-disk polarity is fixed
/// (1 = white per WAP-237 §8.4) but callers may want the decoded
/// plane in either polarity to match downstream image-buffer
/// conventions without having to re-walk the plane themselves.
///
/// Both variants share the same `stride = ceil(width / 8)`,
/// MSB-first bit-order, row-padded layout; the only difference is
/// the meaning of a `1` bit.
///
/// The standalone [`crate::parse_wbmp`] entry point always returns
/// [`Self::MonoWhite`] (the on-disk layout). Callers that want the
/// inverted polarity opt in via [`crate::parse_wbmp_as`] /
/// [`crate::parse_wbmp_as_with_limits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WbmpPixelFormat {
    /// 1 bit per pixel, MSB-first packed, **1 = white, 0 = black**
    /// per WAP-237 §8.4. Rows are padded to a byte boundary —
    /// `stride = ceil(width / 8)`. Maps to `oxideav_core::PixelFormat::MonoWhite`
    /// when the `registry` feature is on.
    MonoWhite,
    /// 1 bit per pixel, MSB-first packed, **1 = black, 0 = white**
    /// (the polarity inverse of [`Self::MonoWhite`]). Produced only
    /// when the caller explicitly opts in via
    /// [`crate::parse_wbmp_as`] / [`crate::parse_wbmp_as_with_limits`].
    /// Padding bits in the last byte of every row are zero (so they
    /// stay distinguishable from real `1`-bit black pixels on
    /// inspection). Maps to `oxideav_core::PixelFormat::MonoBlack`
    /// when the `registry` feature is on.
    MonoBlack,
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
    /// Pixel layout the planes carry. [`WbmpPixelFormat::MonoWhite`]
    /// for [`crate::parse_wbmp`] (the on-disk polarity);
    /// [`WbmpPixelFormat::MonoBlack`] when the caller opts in via
    /// [`crate::parse_wbmp_as`].
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

/// Byte-level layout of a single packed mono plane, derived once from
/// `(width, height)` and reused by every per-row operation that needs
/// the stride, the total packed-buffer size, or the trailing-padding
/// bit mask.
///
/// Before this typed primitive existed, four call sites
/// (`decoder::parse_wbmp_inner`, `decoder::invert_plane_in_place`,
/// `encoder::encode_wbmp`, encoder `MonoBlack` ingress) each
/// independently re-derived `stride = ceil(width / 8)`,
/// `total_bytes = stride * height` (with a `checked_mul` to guard the
/// `usize` overflow path), and — in two of those sites — the
/// per-row last-byte padding mask `0xFF << (8 * stride - width)`.
/// Pulling the three quantities into one struct removes the
/// duplicated arithmetic and gives the polarity-flip + truncation-mask
/// branches a single canonical mask byte to AND against the last byte
/// of every row.
///
/// `last_byte_pad_mask` is `0xFF` for widths that are an exact
/// multiple of 8 — i.e. no padding to mask — so callers can apply
/// it unconditionally without a `pad_bits > 0` guard. For widths that
/// leave 1..=7 padding bits in the last byte, the mask zeros those
/// trailing bits while preserving the leading payload bits
/// (e.g. `width = 11` → `stride = 2`, `pad_bits = 5`,
/// `last_byte_pad_mask = 0b1110_0000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneLayout {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Bytes per row — `ceil(width / 8)`.
    pub stride: usize,
    /// `stride * height` — total bytes the packed plane occupies.
    pub total_bytes: usize,
    /// Mask to AND with the last byte of every row to zero any
    /// trailing padding bits. `0xFF` when `width % 8 == 0` (no
    /// padding), `0xFF << (8 * stride - width)` otherwise.
    pub last_byte_pad_mask: u8,
}

impl PlaneLayout {
    /// Derive the layout for an image of the given pixel dimensions.
    ///
    /// Returns an error message if `stride * height` would overflow
    /// `usize` (the only failure path: the dimensions themselves are
    /// validated by the header parser, so this constructor is a thin
    /// arithmetic guard for the final allocation-size computation).
    ///
    /// Both `width` and `height` are accepted as-is; the caller is
    /// expected to reject zero dimensions through the surrounding
    /// header parser / encode guards (this struct doesn't enforce that
    /// itself — a `width = 0` layout has `stride = 0` and
    /// `total_bytes = 0` rather than an error, which is what the
    /// downstream encoder length check wants to see).
    pub fn new(width: u32, height: u32) -> core::result::Result<Self, &'static str> {
        let stride = (width as usize).div_ceil(8);
        let total_bytes = stride
            .checked_mul(height as usize)
            .ok_or("WBMP: width * height overflows usize")?;
        // pad_bits is 0..=7 (stride * 8 - width >= 0 since stride >= ceil(width/8)).
        let pad_bits = stride.saturating_mul(8).saturating_sub(width as usize);
        let last_byte_pad_mask: u8 = if pad_bits == 0 {
            0xFF
        } else {
            // 1 <= pad_bits <= 7 → shifting a u8 by pad_bits is well-defined.
            0xFFu8 << pad_bits
        };
        Ok(Self {
            width,
            height,
            stride,
            total_bytes,
            last_byte_pad_mask,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_byte_aligned_widths_have_full_mask() {
        // width = 8 / 16 / 24 / 256: every row ends on a byte
        // boundary, mask must be 0xFF (no-op AND).
        for w in [8u32, 16, 24, 256, 1024] {
            let l = PlaneLayout::new(w, 1).unwrap();
            assert_eq!(l.stride, (w as usize) / 8);
            assert_eq!(l.last_byte_pad_mask, 0xFF, "width {w}");
        }
    }

    #[test]
    fn layout_partial_byte_widths_mask_trailing_padding() {
        // width = 11 → stride 2, pad 5 → mask 0b1110_0000 (0xE0).
        let l = PlaneLayout::new(11, 3).unwrap();
        assert_eq!(l.stride, 2);
        assert_eq!(l.total_bytes, 6);
        assert_eq!(l.last_byte_pad_mask, 0xE0);

        // width = 1 → stride 1, pad 7 → mask 0b1000_0000 (0x80).
        let l = PlaneLayout::new(1, 1).unwrap();
        assert_eq!(l.last_byte_pad_mask, 0x80);

        // width = 9 → stride 2, pad 7 → mask 0b1000_0000.
        let l = PlaneLayout::new(9, 1).unwrap();
        assert_eq!(l.last_byte_pad_mask, 0x80);

        // width = 15 → stride 2, pad 1 → mask 0b1111_1110 (0xFE).
        let l = PlaneLayout::new(15, 1).unwrap();
        assert_eq!(l.last_byte_pad_mask, 0xFE);
    }

    #[test]
    fn layout_zero_dimension_does_not_error() {
        // The constructor doesn't enforce non-zero dimensions — the
        // surrounding header parser / encoder guards do. A zero
        // dimension lands as a zero-byte layout rather than an error
        // so the caller can pass through to its own validation step.
        let l = PlaneLayout::new(0, 16).unwrap();
        assert_eq!(l.stride, 0);
        assert_eq!(l.total_bytes, 0);

        let l = PlaneLayout::new(16, 0).unwrap();
        assert_eq!(l.total_bytes, 0);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn layout_overflow_fails_cleanly() {
        // u32::MAX width × u32::MAX height: stride is ~536M, height is
        // ~4G; product is ~2.3e18. On a 32-bit target that overflows
        // usize::MAX (~4.3e9); on 64-bit it doesn't (usize::MAX is
        // ~1.84e19) so the constructor returns Ok. We only assert the
        // error path on platforms where the checked_mul actually fires
        // — the guard's purpose is to keep 32-bit usize safe, since
        // 64-bit usize trivially absorbs every reachable (u32 × u32)
        // product.
        let r = PlaneLayout::new(u32::MAX, u32::MAX);
        assert!(r.is_err(), "expected overflow error on u32::MAX × u32::MAX");
    }

    #[test]
    fn layout_total_bytes_matches_row_stride_times_height() {
        // The total_bytes field must equal what
        // `WbmpImage::row_stride(width) * (height as usize)` produces
        // for non-overflowing inputs.
        for (w, h) in [(8u32, 8u32), (11, 3), (320, 240), (159, 33), (1024, 1024)] {
            let l = PlaneLayout::new(w, h).unwrap();
            let expected = WbmpImage::row_stride(w) * (h as usize);
            assert_eq!(l.total_bytes, expected, "({w}, {h})");
        }
    }
}
