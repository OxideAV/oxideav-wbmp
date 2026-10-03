//! Decode-side limits ([`DecodeOptions`]) and encode-side knobs
//! ([`EncodeOptions`], [`Quantize`]) of the standalone API.

use crate::error::{Result, WbmpError};
use crate::ext::ExtFields;
use crate::image::PixelFormat;

/// Limits and strictness for [`crate::decode_with`] /
/// [`crate::decode_all_with`].
///
/// Every limit is checked against the header **before** any pixel
/// buffer is allocated, so a hostile header fails with
/// [`WbmpError::LimitExceeded`] instead of committing memory. The
/// defaults are: no dimension / pixel-count limit, decoded plane
/// capped at [`DecodeOptions::DEFAULT_MAX_BYTES`] (1 GiB), `strict =
/// false`, native polarity.
///
/// `max_bytes` bounds **one** packed plane (`ceil(width / 8) ×
/// height`); a stream's animated sub-images (at most 15, §4.5.1) share
/// the main image's dimensions, so [`crate::decode_all`] allocates at
/// most 16 × `max_bytes`.
///
/// `strict` selects Type-0 wire conformance:
///
/// | | lenient (`false`, the default) | `strict` |
/// |---|---|---|
/// | `FixHeaderField` | honoured per §4.4.1: when its presence bit is set the `ExtFields` region is parsed and skipped before `Width` (a non-conformant Type-0 producer), surfaced through [`crate::info`] | must be exactly `0x00` (§4.5.1: "Extension headers MUST NOT be presented in this format") |
/// | MBIs (`Type`, `Width`, `Height`) | a bounded run of redundant leading `0x80` octets is tolerated | shortest encoding required (§4.3.1) |
/// | Trailing bytes after the last frame | ignored | ignored |
///
/// Both modes reject a non-zero `Type` (`Unsupported`), a zero
/// dimension and a truncated main image (`InvalidData`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Reject images wider than this (pixels).
    pub max_width: Option<u32>,
    /// Reject images taller than this (pixels).
    pub max_height: Option<u32>,
    /// Reject images with more than this many pixels (`width ×
    /// height`).
    pub max_pixels: Option<u64>,
    /// Reject images whose packed plane would exceed this many bytes
    /// (`ceil(width / 8) × height`, per frame).
    pub max_bytes: Option<u64>,
    /// Type-0 wire conformance (see the type docs).
    pub strict: bool,
    /// WBMP extra: the polarity of the returned plane. The default,
    /// [`PixelFormat::MonoBlack`], is the wire layout copied verbatim;
    /// [`PixelFormat::MonoWhite`] flips every bit during the decode-time
    /// row copy (padding bits stay zero).
    pub format: PixelFormat,
}

impl DecodeOptions {
    /// Default [`Self::max_bytes`]: 1 GiB of packed plane.
    pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;

    /// The defaults (see the type docs).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or lift with `None`) the width limit.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set (or lift with `None`) the height limit.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set (or lift with `None`) the pixel-count limit.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set (or lift with `None`) the packed-plane-bytes limit.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set strict mode (see the type docs).
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Set the polarity of the returned plane.
    pub fn with_format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    /// Lift every limit (`max_*` all `None`).
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self
    }

    /// Check a header's geometry against the limits. `bytes` is the
    /// packed plane size the geometry implies.
    pub(crate) fn check(&self, width: u32, height: u32, bytes: u64) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(WbmpError::limit_exceeded(format!(
                    "WBMP: width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(WbmpError::limit_exceeded(format!(
                    "WBMP: height {height} exceeds max_height {m}"
                )));
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        if let Some(m) = self.max_pixels {
            if pixels > m {
                return Err(WbmpError::limit_exceeded(format!(
                    "WBMP: {pixels} pixels exceed max_pixels {m}"
                )));
            }
        }
        if let Some(m) = self.max_bytes {
            if bytes > m {
                return Err(WbmpError::limit_exceeded(format!(
                    "WBMP: pixel-data size {bytes} exceeds max_bytes {m}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: None,
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
            format: PixelFormat::NATIVE,
        }
    }
}

/// How 8-bit grey (or RGB luma) samples become 1-bit pixels in
/// [`crate::encode_rgb8`] / [`crate::encode_rgba8`] /
/// [`crate::encode_gray8`] / [`crate::WbmpImage::from_gray8`].
///
/// Native 1-bit input ([`crate::encode`] of a [`crate::WbmpImage`]) is
/// never quantised; this only applies to continuous-tone input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Quantize {
    /// Hard threshold: samples `>= t` become white, samples below
    /// become black. `Threshold(128)` is the standard mid-grey cutoff
    /// and the default.
    Threshold(u8),
    /// Floyd–Steinberg error diffusion (7/16, 3/16, 5/16, 1/16 to the
    /// four forward neighbours, i16 accumulator, round-to-nearest
    /// division, mid-grey decision at 128): photographic mid-tones
    /// land as stippled patterns that preserve the local average
    /// luminance instead of collapsing to flat regions. Reference:
    /// R. W. Floyd and L. Steinberg, "An adaptive algorithm for
    /// spatial greyscale", Proc. SID 17/2 (1976), pp. 75–77.
    Dither,
}

impl Quantize {
    /// The default rule: [`Quantize::Threshold`]`(128)`.
    pub const DEFAULT: Self = Self::Threshold(128);
}

impl Default for Quantize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Encoder knobs for [`crate::encode`] / [`crate::encode_rgb8`] /
/// [`crate::encode_rgba8`] / [`crate::encode_gray8`] /
/// [`crate::encode_frames`].
///
/// WBMP Type 0 has no compression and no quality axis; the options
/// are the continuous-tone quantisation rule and the (deliberately
/// non-conformant, interop-testing only) extension-header region.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// How 8-bit input is reduced to 1 bit (ignored for native 1-bit
    /// images). Default [`Quantize::DEFAULT`] (threshold 128).
    pub quantize: Quantize,
    /// Extension-header region to write (§4.4.1), `None` (the default)
    /// for a conformant Type-0 header (`FixHeaderField == 0x00`).
    /// `Some` synthesises a `FixHeaderField` with the presence flag set
    /// and serialises the region before `Width` — WBMP Type 0
    /// conformantly forbids this (§4.5.1), so the stream only decodes
    /// through the lenient path ([`crate::decode`] with `strict =
    /// false`), never through a strict decoder.
    pub ext_fields: Option<ExtFields>,
    /// With `ext_fields`: validate a Type-11 region's parameter
    /// character classes (§4.4.3 ABNF: identifier US-ASCII `CHAR`,
    /// value `ALPHA / DIGIT`) before emitting. No effect otherwise.
    pub strict: bool,
}

impl EncodeOptions {
    /// The defaults (threshold 128, conformant header).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the quantisation rule for 8-bit input.
    pub fn with_quantize(mut self, quantize: Quantize) -> Self {
        self.quantize = quantize;
        self
    }

    /// Shorthand for [`Self::with_quantize`]`(Quantize::Threshold(t))`.
    pub fn with_threshold(self, threshold: u8) -> Self {
        self.with_quantize(Quantize::Threshold(threshold))
    }

    /// Shorthand for [`Self::with_quantize`]`(Quantize::Dither)`.
    pub fn with_dither(self) -> Self {
        self.with_quantize(Quantize::Dither)
    }

    /// Set (or clear) the extension-header region.
    pub fn with_ext_fields(mut self, ext_fields: impl Into<Option<ExtFields>>) -> Self {
        self.ext_fields = ext_fields.into();
        self
    }

    /// Set the Type-11 character-class validation flag.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_fire_in_order() {
        let o = DecodeOptions::default()
            .with_max_width(10u32)
            .with_max_height(10u32)
            .with_max_pixels(50u64)
            .with_max_bytes(100u64);
        assert!(o.check(5, 5, 75).is_ok());
        assert!(matches!(
            o.check(11, 1, 1),
            Err(WbmpError::LimitExceeded(_))
        ));
        assert!(matches!(
            o.check(1, 11, 1),
            Err(WbmpError::LimitExceeded(_))
        ));
        assert!(matches!(o.check(8, 8, 1), Err(WbmpError::LimitExceeded(_))));
        assert!(matches!(
            o.check(5, 5, 101),
            Err(WbmpError::LimitExceeded(_))
        ));
        assert!(o.unlimited().check(u32::MAX, u32::MAX, u64::MAX).is_ok());
    }

    #[test]
    fn defaults() {
        let d = DecodeOptions::default();
        assert_eq!(d.max_width, None);
        assert_eq!(d.max_height, None);
        assert_eq!(d.max_pixels, None);
        assert_eq!(d.max_bytes, Some(1 << 30));
        assert!(!d.strict);
        assert_eq!(d.format, PixelFormat::MonoBlack);
        let e = EncodeOptions::default();
        assert_eq!(e.quantize, Quantize::Threshold(128));
        assert_eq!(e.ext_fields, None);
        assert!(!e.strict);
        assert_eq!(
            EncodeOptions::new()
                .with_dither()
                .with_threshold(7)
                .quantize,
            Quantize::Threshold(7)
        );
    }
}
