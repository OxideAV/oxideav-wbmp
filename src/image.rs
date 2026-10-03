//! The standalone image types: the shapes every `oxideav-<format>`
//! image crate shares (`IMAGE_CRATE_API`), specialised for WBMP.
//!
//! * [`WbmpImage`] — the native-layout image [`crate::decode`] returns
//!   and [`crate::encode`] consumes: dimensions, a [`PixelFormat`]
//!   tag, one packed 1-bit [`Plane`], [`ColorInfo`] and (always empty
//!   for WBMP) [`Metadata`].
//! * [`RgbImage`] / [`RgbaImage`] — the tightly packed 8-bit raw
//!   paths ([`crate::decode_rgb8`] / [`crate::decode_rgba8`],
//!   [`WbmpImage::to_rgb8`] / [`WbmpImage::to_rgba8`]).
//! * [`ImageInfo`] — what [`crate::info`] reads from the header.
//! * [`Frame`] — one element of [`crate::decode_all`] (the main image
//!   plus the WAP-237 §4.5.1 animated sub-images).
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate can be built with the default `registry` feature off — i.e.
//! without depending on `oxideav-core` at all. When the `registry`
//! feature is on the `crate::registry` module exposes the
//! [`WbmpPixelFormat`] ↔ `oxideav_core::PixelFormat` mapping, the frame
//! bridge and the `From<WbmpError> for oxideav_core::Error` impl.

use std::borrow::Cow;
use std::time::Duration;

use crate::error::{Result, WbmpError};
use crate::ext::ExtFields;
use crate::options::Quantize;

// ---------------------------------------------------------------------------
// Pixel format
// ---------------------------------------------------------------------------

/// Pixel layouts the standalone `oxideav-wbmp` API can produce /
/// consume. WBMP Type 0 carries monochrome 1-bit-per-pixel data.
///
/// Variant names — and their bit polarity — mirror
/// `oxideav_core::PixelFormat` exactly, so the `registry` conversion
/// layer is a 1:1 match:
///
/// | Variant | bit `0` | bit `1` | Role |
/// |---|---|---|---|
/// | [`MonoBlack`](Self::MonoBlack) | black | white | the **native** layout — identical to the WBMP wire bytes (WAP-237 §4.5.1: "white=1, black=0") |
/// | [`MonoWhite`](Self::MonoWhite) | white | black | the inverse polarity, available through [`WbmpImage::into_format`] / [`crate::DecodeOptions::format`] |
///
/// Both variants share the same `stride = ceil(width / 8)`, MSB-first
/// bit order (the high bit is the left-most pixel) and byte-padded
/// rows; the padding bits of every row are zero in both polarities.
///
/// Before the image-crate API contract this crate named the native
/// layout `MonoWhite` ("1 = white"), the opposite of the core enum's
/// definition; the registry path therefore emitted inverted frames.
/// The variants now carry the core meaning (see the CHANGELOG).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WbmpPixelFormat {
    /// 1 bit per pixel, MSB-first packed, **0 = black, 1 = white** —
    /// the WBMP wire polarity and the layout [`crate::decode`]
    /// returns. Maps to `oxideav_core::PixelFormat::MonoBlack`.
    MonoBlack,
    /// 1 bit per pixel, MSB-first packed, **0 = white, 1 = black** —
    /// the inverse of the wire polarity. Maps to
    /// `oxideav_core::PixelFormat::MonoWhite`.
    MonoWhite,
}

/// The contract name for [`WbmpPixelFormat`].
pub type PixelFormat = WbmpPixelFormat;

impl WbmpPixelFormat {
    /// The layout [`crate::decode`] returns: [`Self::MonoBlack`], the
    /// WBMP wire polarity.
    pub const NATIVE: Self = Self::MonoBlack;

    /// The bit value that means "white" in this layout (`1` for
    /// `MonoBlack`, `0` for `MonoWhite`).
    pub const fn white_bit(self) -> u8 {
        match self {
            Self::MonoBlack => 1,
            Self::MonoWhite => 0,
        }
    }

    /// The other polarity.
    pub const fn inverted(self) -> Self {
        match self {
            Self::MonoBlack => Self::MonoWhite,
            Self::MonoWhite => Self::MonoBlack,
        }
    }

    /// Always `false`: WBMP has no alpha mechanism.
    pub const fn has_alpha(self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Plane / colour / metadata
// ---------------------------------------------------------------------------

/// One pixel plane: `stride` bytes per row, `data` holding at least
/// `stride × (height − 1) + ceil(width / 8)` bytes (rows may carry
/// padding past the packed width). WBMP layouts are packed 1-bit, so a
/// [`WbmpImage`] has exactly one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Plane {
    /// Bytes per row — `ceil(width / 8)` for a decoder-produced plane.
    pub stride: usize,
    /// Row-major bytes. Bits within each byte are MSB-first; trailing
    /// bits in the last byte of every row are zero-padded by the
    /// encoder (and ignored by the decoder).
    pub data: Vec<u8>,
}

impl Plane {
    /// Wrap a plane buffer with its row stride.
    pub fn new(stride: usize, data: Vec<u8>) -> Self {
        Self { stride, data }
    }
}

/// Pre-contract name of [`Plane`].
#[deprecated(note = "use oxideav_wbmp::Plane (IMAGE_CRATE_API)")]
pub type WbmpPlane = Plane;

/// Nominal sample range (H.273 `VideoFullRangeFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// No range was signalled.
    #[default]
    Unspecified,
    /// Limited (video / studio) range: `VideoFullRangeFlag == 0`.
    Limited,
    /// Full (PC) range: `VideoFullRangeFlag == 1`.
    Full,
}

/// Colour signalling of an image: the sample range plus the H.273
/// `ColourPrimaries` / `TransferCharacteristics` /
/// `MatrixCoefficients` code points (`2` = unspecified).
///
/// WBMP carries no colour information of any kind, so every decoded
/// image gets [`ColorInfo::wbmp_default`]: a 1-bit sample is one of the
/// two extremes of a full-range achromatic signal (`range` Full,
/// `matrix` 0 = identity / RGB), with primaries and transfer
/// unspecified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ColorInfo {
    /// Sample range.
    pub range: ColorRange,
    /// H.273 `ColourPrimaries` code point (`2` = unspecified).
    pub primaries: u8,
    /// H.273 `TransferCharacteristics` code point (`2` = unspecified).
    pub transfer: u8,
    /// H.273 `MatrixCoefficients` code point (`0` = identity / RGB).
    pub matrix: u8,
}

impl ColorInfo {
    /// H.273 "unspecified" code point.
    pub const UNSPECIFIED: u8 = 2;
    /// H.273 `MatrixCoefficients` identity (RGB / GBR) code point.
    pub const MATRIX_IDENTITY: u8 = 0;

    /// Build a description from its four parts.
    pub const fn new(range: ColorRange, primaries: u8, transfer: u8, matrix: u8) -> Self {
        Self {
            range,
            primaries,
            transfer,
            matrix,
        }
    }

    /// Every field unspecified.
    pub const fn unspecified() -> Self {
        Self::new(
            ColorRange::Unspecified,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
        )
    }

    /// WBMP's documented default: full range, identity matrix,
    /// primaries and transfer unspecified (the format signals no
    /// colour; black and white are "the two states of pixel off and
    /// on", WAP-237 §4).
    pub const fn wbmp_default() -> Self {
        Self::new(
            ColorRange::Full,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::MATRIX_IDENTITY,
        )
    }

    /// Set the range.
    pub fn with_range(mut self, range: ColorRange) -> Self {
        self.range = range;
        self
    }

    /// Set the primaries code point.
    pub fn with_primaries(mut self, primaries: u8) -> Self {
        self.primaries = primaries;
        self
    }

    /// Set the transfer code point.
    pub fn with_transfer(mut self, transfer: u8) -> Self {
        self.transfer = transfer;
        self
    }

    /// Set the matrix code point.
    pub fn with_matrix(mut self, matrix: u8) -> Self {
        self.matrix = matrix;
        self
    }

    /// `true` when both primaries and transfer are specified (`!= 2`).
    pub fn is_specified(&self) -> bool {
        self.primaries != Self::UNSPECIFIED && self.transfer != Self::UNSPECIFIED
    }
}

impl Default for ColorInfo {
    /// [`ColorInfo::wbmp_default`].
    fn default() -> Self {
        Self::wbmp_default()
    }
}

/// The metadata blobs every image crate surfaces: an ICC profile, an
/// Exif payload, an XMP packet and a file gamma.
///
/// WBMP has no metadata mechanism (its extension headers carry
/// opaque reserved bits or short parameter/value pairs, never colour
/// or ICC data), so every field is `None` on a decoded image and the
/// encoder ignores (cannot carry) whatever a caller sets. The type
/// exists so [`WbmpImage`] has the same shape as every other image
/// crate's image.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Metadata {
    /// ICC profile bytes. Always `None` from the decoder.
    pub icc: Option<Vec<u8>>,
    /// Exif payload. Always `None` from the decoder.
    pub exif: Option<Vec<u8>>,
    /// XMP packet. Always `None` from the decoder.
    pub xmp: Option<Vec<u8>>,
    /// File gamma. Always `None` from the decoder.
    pub gamma: Option<f32>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or clear) the ICC profile.
    pub fn with_icc(mut self, icc: impl Into<Option<Vec<u8>>>) -> Self {
        self.icc = icc.into();
        self
    }

    /// Set (or clear) the Exif payload.
    pub fn with_exif(mut self, exif: impl Into<Option<Vec<u8>>>) -> Self {
        self.exif = exif.into();
        self
    }

    /// Set (or clear) the XMP packet.
    pub fn with_xmp(mut self, xmp: impl Into<Option<Vec<u8>>>) -> Self {
        self.xmp = xmp.into();
        self
    }

    /// Set (or clear) the file gamma.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// `true` when no field is set.
    pub fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none() && self.xmp.is_none() && self.gamma.is_none()
    }
}

// ---------------------------------------------------------------------------
// WbmpImage
// ---------------------------------------------------------------------------

/// Decoded WBMP image in its native layout, as returned by
/// [`crate::decode`] and consumed by [`crate::encode`].
///
/// `planes` holds exactly one packed 1-bit plane (stride
/// `ceil(width / 8)` from the decoder); `format` is
/// [`PixelFormat::MonoBlack`] from the decoder (the wire polarity) or
/// [`PixelFormat::MonoWhite`] after [`WbmpImage::into_format`]; `color`
/// is [`ColorInfo::wbmp_default`]; `metadata` is always empty. WBMP
/// has no palette, so there is no `palette` field.
///
/// Construct with [`WbmpImage::new`] / [`WbmpImage::from_bits`] /
/// [`WbmpImage::from_gray8`] / [`WbmpImage::from_rgb8`] /
/// [`WbmpImage::from_rgba8`], which validate the plane geometry so an
/// inconsistent image cannot exist and [`WbmpImage::to_rgb8`] /
/// [`WbmpImage::to_rgba8`] are infallible.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WbmpImage {
    /// Picture width in pixels (≥ 1).
    pub width: u32,
    /// Picture height in pixels (≥ 1).
    pub height: u32,
    /// Pixel layout (bit polarity) the plane carries.
    pub format: PixelFormat,
    /// One [`Plane`] — WBMP only ever ships a single packed plane.
    pub planes: Vec<Plane>,
    /// Colour signalling — always [`ColorInfo::wbmp_default`] from the
    /// decoder (WBMP signals none).
    pub color: ColorInfo,
    /// ICC / Exif / XMP / gamma — always empty for WBMP.
    pub metadata: Metadata,
}

impl WbmpImage {
    /// `ceil(width / 8)` — number of bytes a single row occupies in
    /// the packed plane.
    pub fn row_stride(width: u32) -> usize {
        (width as usize).div_ceil(8)
    }

    /// Assemble an image from its geometry, layout and planes (exactly
    /// one for WBMP). Colour is [`ColorInfo::wbmp_default`] and
    /// metadata empty; the `with_*` builders fill those in.
    ///
    /// Validates the geometry and returns [`WbmpError::InvalidData`]
    /// when `width` or `height` is `0` (WBMP cannot carry an empty
    /// image), when there is not exactly one plane, when the plane's
    /// `stride` is below `ceil(width / 8)`, or when its `data` is
    /// shorter than `stride × (height − 1) + ceil(width / 8)`.
    pub fn new(width: u32, height: u32, format: PixelFormat, planes: Vec<Plane>) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(WbmpError::invalid(format!(
                "WBMP: zero dimension (width={width}, height={height})"
            )));
        }
        if planes.len() != 1 {
            return Err(WbmpError::invalid(format!(
                "WBMP: expected exactly one packed plane, got {}",
                planes.len()
            )));
        }
        let row_bytes = Self::row_stride(width);
        let plane = &planes[0];
        if plane.stride < row_bytes {
            return Err(WbmpError::invalid(format!(
                "WBMP: stride {} below packed row size {row_bytes}",
                plane.stride
            )));
        }
        let needed = plane
            .stride
            .checked_mul(height as usize - 1)
            .and_then(|n| n.checked_add(row_bytes))
            .ok_or_else(|| WbmpError::unsupported("WBMP: plane size overflows usize"))?;
        if plane.data.len() < needed {
            return Err(WbmpError::invalid(format!(
                "WBMP: plane holds {} bytes, geometry needs {needed}",
                plane.data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            format,
            planes,
            color: ColorInfo::wbmp_default(),
            metadata: Metadata::default(),
        })
    }

    /// One packed plane with an explicit row stride (`stride ≥
    /// ceil(width / 8)`). Same validation as [`Self::new`].
    pub fn packed(
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        data: Vec<u8>,
    ) -> Result<Self> {
        Self::new(width, height, format, vec![Plane::new(stride, data)])
    }

    /// A native-layout ([`PixelFormat::MonoBlack`], 1 = white) image
    /// from `ceil(width / 8) × height` tightly packed bits — the shape
    /// the WBMP wire format carries. More bytes are tolerated; fewer
    /// are [`WbmpError::InvalidData`].
    pub fn from_bits(width: u32, height: u32, bits: Vec<u8>) -> Result<Self> {
        Self::packed(
            width,
            height,
            PixelFormat::NATIVE,
            Self::row_stride(width),
            bits,
        )
    }

    /// Quantise `width × height` 8-bit grey samples (one byte per
    /// pixel, no row padding) to a native-layout 1-bit image with the
    /// given [`Quantize`] rule. Fewer bytes than `width × height` is
    /// [`WbmpError::InvalidData`]; more are ignored.
    pub fn from_gray8(width: u32, height: u32, gray: &[u8], quantize: Quantize) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(WbmpError::invalid(format!(
                "WBMP: zero dimension (width={width}, height={height})"
            )));
        }
        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| WbmpError::unsupported("WBMP: width × height overflows usize"))?;
        if gray.len() < pixel_count {
            return Err(WbmpError::invalid(format!(
                "WBMP: gray buffer holds {} bytes, {width}×{height} needs {pixel_count}",
                gray.len()
            )));
        }
        let bits = crate::quantize::quantize_gray8(width, height, &gray[..pixel_count], quantize);
        Self::from_bits(width, height, bits)
    }

    /// Tightly packed 8-bit RGB (`3 × width × height` bytes) to a
    /// native 1-bit image: each pixel's luma `Y = (299 R + 587 G +
    /// 114 B + 500) / 1000` (Rec. 601 weights, rounded) is thresholded
    /// at 128 ([`Quantize::DEFAULT`]). [`crate::encode_rgb8`] offers the
    /// dithering alternative through [`crate::EncodeOptions`]. Fewer
    /// bytes than the geometry needs is [`WbmpError::InvalidData`].
    pub fn from_rgb8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::from_rgb8_with(width, height, &data, Quantize::DEFAULT)
    }

    /// Tightly packed 8-bit RGBA (`4 × width × height` bytes) to a
    /// native 1-bit image. Alpha is **dropped** (WBMP has no alpha
    /// mechanism; a transparent pixel keeps its colour samples), then
    /// the pixels quantise as in [`Self::from_rgb8`].
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::from_rgba8_with(width, height, &data, Quantize::DEFAULT)
    }

    /// [`Self::from_rgb8`] with an explicit quantisation rule.
    pub fn from_rgb8_with(width: u32, height: u32, rgb: &[u8], quantize: Quantize) -> Result<Self> {
        let gray = rgb_to_gray8(width, height, rgb, 3)?;
        Self::from_gray8(width, height, &gray, quantize)
    }

    /// [`Self::from_rgba8`] with an explicit quantisation rule (alpha
    /// dropped).
    pub fn from_rgba8_with(
        width: u32,
        height: u32,
        rgba: &[u8],
        quantize: Quantize,
    ) -> Result<Self> {
        let gray = rgb_to_gray8(width, height, rgba, 4)?;
        Self::from_gray8(width, height, &gray, quantize)
    }

    /// Set the colour signalling. WBMP cannot carry it; the encoder
    /// ignores it.
    pub fn with_color(mut self, color: ColorInfo) -> Self {
        self.color = color;
        self
    }

    /// Set the metadata. WBMP cannot carry any of it; the encoder
    /// ignores it.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pixel layout (bit polarity).
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Row stride in bytes of the pixel plane.
    pub fn stride(&self) -> usize {
        self.planes.first().map(|p| p.stride).unwrap_or(0)
    }

    /// Always `false`: WBMP has no alpha.
    pub fn has_alpha(&self) -> bool {
        false
    }

    /// `true` when the plane is tightly packed (`stride == ceil(width /
    /// 8)` and no trailing bytes) — always the case for a
    /// decoder-produced image.
    pub fn is_tightly_packed(&self) -> bool {
        let row = Self::row_stride(self.width);
        self.planes
            .first()
            .is_some_and(|p| p.stride == row && p.data.len() == row * self.height as usize)
    }

    /// The pixel bytes — `Some` for every WBMP image (one packed
    /// plane). Includes row padding when the plane's stride exceeds
    /// `ceil(width / 8)`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.planes.first().map(|p| p.data.as_slice())
    }

    /// Consume the image and return its plane bytes.
    pub fn into_raw(self) -> Vec<u8> {
        let mut planes = self.planes.into_iter();
        let mut out = planes.next().map(|p| p.data).unwrap_or_default();
        for p in planes {
            out.extend_from_slice(&p.data);
        }
        out
    }

    /// Pixel bytes of the single plane (empty if none).
    pub(crate) fn data(&self) -> &[u8] {
        self.as_bytes().unwrap_or(&[])
    }

    /// `true` when pixel `(x, y)` is white, whatever the polarity.
    /// Out-of-range coordinates read as black.
    pub fn is_white(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let idx = y as usize * self.stride() + (x as usize >> 3);
        let bit = self
            .data()
            .get(idx)
            .map(|b| (b >> (7 - (x & 7))) & 1)
            .unwrap_or(0);
        bit == self.format.white_bit()
    }

    /// Convert to the other polarity in place (every payload bit
    /// flipped, the padding bits of every row re-zeroed, `format`
    /// swapped). Converting to the current `format` is a no-op.
    pub fn into_format(mut self, format: PixelFormat) -> Self {
        self.convert_format(format);
        self
    }

    /// In-place form of [`Self::into_format`].
    pub fn convert_format(&mut self, format: PixelFormat) {
        if self.format == format {
            return;
        }
        let layout = match PlaneLayout::new(self.width, self.height) {
            Ok(l) => l,
            Err(_) => return,
        };
        let height = self.height as usize;
        if let Some(plane) = self.planes.first_mut() {
            let stride = plane.stride;
            for y in 0..height {
                let Some(row) = plane
                    .data
                    .get_mut(y * stride..)
                    .and_then(|r| r.get_mut(..layout.stride))
                else {
                    break;
                };
                for b in row.iter_mut() {
                    *b = !*b;
                }
                if let Some(last) = row.last_mut() {
                    *last &= layout.last_byte_pad_mask;
                }
            }
        }
        self.format = format;
    }

    /// The pixels as one tightly packed `ceil(width / 8) × height`
    /// buffer in the **wire** polarity ([`PixelFormat::MonoBlack`],
    /// padding bits zero): a borrow when the plane already is that
    /// (the decoder's output), a repacked / re-polarised copy
    /// otherwise.
    pub(crate) fn wire_bits(&self) -> Cow<'_, [u8]> {
        let layout = PlaneLayout::new(self.width, self.height).ok();
        let (row, total, mask) = match layout {
            Some(l) => (l.stride, l.total_bytes, l.last_byte_pad_mask),
            None => return Cow::Owned(Vec::new()),
        };
        let stride = self.stride();
        let src = self.data();
        if self.format == PixelFormat::NATIVE
            && stride == row
            && src.len() == total
            && (mask == 0xFF || src.chunks_exact(row).all(|r| r[row - 1] & !mask == 0))
        {
            return Cow::Borrowed(src);
        }
        let invert = self.format != PixelFormat::NATIVE;
        let mut out = vec![0u8; total];
        for (y, dst) in out.chunks_exact_mut(row).enumerate() {
            if let Some(s) = src.get(y * stride..).and_then(|s| s.get(..row)) {
                dst.copy_from_slice(s);
            }
            if invert {
                for b in dst.iter_mut() {
                    *b = !*b;
                }
            }
            if let Some(last) = dst.last_mut() {
                *last &= mask;
            }
        }
        Cow::Owned(out)
    }

    /// Tightly packed 8-bit grey, one byte per pixel: `255` for a
    /// white pixel, `0` for a black one, whatever the polarity.
    pub fn to_gray8(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = self.stride();
        let src = self.data();
        let white = self.format.white_bit();
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            let row = &mut out[y * w..(y + 1) * w];
            for (x, g) in row.iter_mut().enumerate() {
                let bit = src
                    .get(y * stride + (x >> 3))
                    .map(|b| (b >> (7 - (x & 7))) & 1)
                    .unwrap_or(0);
                *g = if bit == white { 255 } else { 0 };
            }
        }
        out
    }

    /// Tightly packed 8-bit RGB, `3 × width` bytes per row: each pixel
    /// is `0, 0, 0` (black) or `255, 255, 255` (white). Exact.
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.to_gray8()
            .into_iter()
            .flat_map(|g| [g, g, g])
            .collect()
    }

    /// Tightly packed 8-bit RGBA, `4 × width` bytes per row: each pixel
    /// is black or white with alpha `255` (WBMP has no alpha). Exact.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.to_gray8()
            .into_iter()
            .flat_map(|g| [g, g, g, 255])
            .collect()
    }
}

/// `width × height` grey samples from a packed `bpp`-byte-per-pixel RGB
/// (`bpp == 3`) or RGBA (`bpp == 4`, alpha dropped) buffer using the
/// rounded Rec. 601 luma weights.
fn rgb_to_gray8(width: u32, height: u32, data: &[u8], bpp: usize) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "WBMP: zero dimension (width={width}, height={height})"
        )));
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| WbmpError::unsupported("WBMP: width × height overflows usize"))?;
    let needed = pixel_count
        .checked_mul(bpp)
        .ok_or_else(|| WbmpError::unsupported("WBMP: buffer size overflows usize"))?;
    if data.len() < needed {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel buffer holds {} bytes, {width}×{height}×{bpp} needs {needed}",
            data.len()
        )));
    }
    Ok(data[..needed]
        .chunks_exact(bpp)
        .map(|px| luma(px[0], px[1], px[2]))
        .collect())
}

/// Rounded Rec. 601 luma: `(299 R + 587 G + 114 B + 500) / 1000`.
#[inline]
pub(crate) fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000) as u8
}

// ---------------------------------------------------------------------------
// Raw RGB / RGBA
// ---------------------------------------------------------------------------

/// Tightly packed 8-bit RGB image: `width × height × 3` bytes,
/// row-major, no padding. What [`crate::decode_rgb8`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 3` bytes, R, G, B per pixel.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// Wrap a packed RGB buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume and return the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }
}

/// Tightly packed 8-bit RGBA image: `width × height × 4` bytes,
/// row-major, no padding. What [`crate::decode_rgba8`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 4` bytes, R, G, B, A per pixel.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Wrap a packed RGBA buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume and return the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }
}

// ---------------------------------------------------------------------------
// ImageInfo / Frame
// ---------------------------------------------------------------------------

/// What [`crate::info`] reads from the header without decoding pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImageInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Native layout [`crate::decode`] would return — always
    /// [`PixelFormat::MonoBlack`].
    pub format: PixelFormat,
    /// Number of images in the file: the main image plus the WAP-237
    /// §4.5.1 animated sub-images that fit in the buffer after it
    /// (`1..=16`). Arithmetic on the header and the buffer length — no
    /// pixel is read.
    pub frames: u32,
    /// Always `false` (WBMP has no alpha).
    pub has_alpha: bool,
    /// Always [`ColorInfo::wbmp_default`] (WBMP signals no colour).
    pub color: ColorInfo,
    /// Always `false` (WBMP carries no ICC profile).
    pub has_icc: bool,
    /// Always `false` (WBMP carries no Exif).
    pub has_exif: bool,
    /// Always `false` (WBMP carries no XMP).
    pub has_xmp: bool,
    /// WBMP extra: the raw `FixHeaderField` octet (§4.4.2). `0x00` in
    /// a conformant Type-0 file.
    pub fix_header: u8,
    /// WBMP extra: the extension-header region (§4.4.1 / §4.4.3) when
    /// the `FixHeaderField` presence bit is set — a non-conformant
    /// Type-0 producer only; `None` otherwise.
    pub ext_fields: Option<ExtFields>,
    /// WBMP extra: byte offset of the first main-image octet (just past
    /// the header, including any `ExtFields`).
    pub data_offset: usize,
}

impl ImageInfo {
    /// A conformant single-image Type-0 header record for `width ×
    /// height` whose pixel data starts at `data_offset`: `frames` 1,
    /// `fix_header` 0, no extension fields, WBMP's default colour.
    pub fn new(width: u32, height: u32, data_offset: usize) -> Self {
        Self {
            width,
            height,
            format: PixelFormat::NATIVE,
            frames: 1,
            has_alpha: false,
            color: ColorInfo::wbmp_default(),
            has_icc: false,
            has_exif: false,
            has_xmp: false,
            fix_header: 0,
            ext_fields: None,
            data_offset,
        }
    }

    /// Set the frame count.
    pub fn with_frames(mut self, frames: u32) -> Self {
        self.frames = frames;
        self
    }

    /// Set the raw `FixHeaderField` octet and the parsed extension
    /// region.
    pub fn with_ext(mut self, fix_header: u8, ext_fields: Option<ExtFields>) -> Self {
        self.fix_header = fix_header;
        self.ext_fields = ext_fields;
        self
    }
}

/// One image of a WBMP stream, as returned by [`crate::decode_all`].
///
/// WAP-237 defines no timing for its animated sub-images ("It is User
/// Agent dependent how those animated images are processed", §4.5.1)
/// and its extension headers carry no timing parameter, so `delay` is
/// always `None`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Frame {
    /// The decoded image (native layout, same dimensions for every
    /// frame of a stream).
    pub image: WbmpImage,
    /// Presentation delay — always `None` for WBMP (no timing in the
    /// format).
    pub delay: Option<Duration>,
    /// WBMP extra: position in the stream — `0` is the main image,
    /// `1..=15` the animated sub-images in stream order.
    pub index: u32,
}

impl Frame {
    /// Wrap an image as frame `index` with no delay.
    pub fn new(image: WbmpImage, index: u32) -> Self {
        Self {
            image,
            delay: None,
            index,
        }
    }
}

// ---------------------------------------------------------------------------
// PlaneLayout (plumbing)
// ---------------------------------------------------------------------------

/// Byte-level layout of a single packed mono plane, derived once from
/// `(width, height)` and reused by every per-row operation that needs
/// the stride, the total packed-buffer size, or the trailing-padding
/// bit mask.
///
/// `last_byte_pad_mask` is `0xFF` for widths that are an exact
/// multiple of 8 — i.e. no padding to mask — so callers can apply
/// it unconditionally without a `pad_bits > 0` guard. For widths that
/// leave 1..=7 padding bits in the last byte, the mask zeros those
/// trailing bits while preserving the leading payload bits
/// (e.g. `width = 11` → `stride = 2`, `pad_bits = 5`,
/// `last_byte_pad_mask = 0b1110_0000`).
#[doc(hidden)]
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
    /// Both `width` and `height` are accepted as-is; a `width = 0`
    /// layout has `stride = 0` and `total_bytes = 0` rather than an
    /// error — the surrounding header parser / encode guards reject
    /// zero dimensions.
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
        for w in [8u32, 16, 24, 256, 1024] {
            let l = PlaneLayout::new(w, 1).unwrap();
            assert_eq!(l.stride, (w as usize) / 8);
            assert_eq!(l.last_byte_pad_mask, 0xFF, "width {w}");
        }
    }

    #[test]
    fn layout_partial_byte_widths_mask_trailing_padding() {
        let l = PlaneLayout::new(11, 3).unwrap();
        assert_eq!(l.stride, 2);
        assert_eq!(l.total_bytes, 6);
        assert_eq!(l.last_byte_pad_mask, 0xE0);
        assert_eq!(PlaneLayout::new(1, 1).unwrap().last_byte_pad_mask, 0x80);
        assert_eq!(PlaneLayout::new(9, 1).unwrap().last_byte_pad_mask, 0x80);
        assert_eq!(PlaneLayout::new(15, 1).unwrap().last_byte_pad_mask, 0xFE);
    }

    #[test]
    fn layout_zero_dimension_does_not_error() {
        let l = PlaneLayout::new(0, 16).unwrap();
        assert_eq!(l.stride, 0);
        assert_eq!(l.total_bytes, 0);
        assert_eq!(PlaneLayout::new(16, 0).unwrap().total_bytes, 0);
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn layout_overflow_fails_cleanly() {
        assert!(PlaneLayout::new(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn layout_total_bytes_matches_row_stride_times_height() {
        for (w, h) in [(8u32, 8u32), (11, 3), (320, 240), (159, 33), (1024, 1024)] {
            let l = PlaneLayout::new(w, h).unwrap();
            assert_eq!(l.total_bytes, WbmpImage::row_stride(w) * (h as usize));
        }
    }

    #[test]
    fn pixel_format_polarity_mirrors_core() {
        assert_eq!(PixelFormat::MonoBlack.white_bit(), 1);
        assert_eq!(PixelFormat::MonoWhite.white_bit(), 0);
        assert_eq!(PixelFormat::NATIVE, PixelFormat::MonoBlack);
        assert_eq!(PixelFormat::MonoBlack.inverted(), PixelFormat::MonoWhite);
        assert!(!PixelFormat::MonoBlack.has_alpha());
    }

    #[test]
    fn new_validates_geometry() {
        assert!(matches!(
            WbmpImage::new(0, 1, PixelFormat::MonoBlack, vec![Plane::new(0, vec![])]),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            WbmpImage::new(1, 1, PixelFormat::MonoBlack, vec![]),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            WbmpImage::new(
                9,
                1,
                PixelFormat::MonoBlack,
                vec![Plane::new(1, vec![0; 2])]
            ),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            WbmpImage::from_bits(9, 2, vec![0; 3]),
            Err(WbmpError::InvalidData(_))
        ));
        let ok = WbmpImage::from_bits(9, 2, vec![0; 4]).unwrap();
        assert!(ok.is_tightly_packed());
        assert_eq!(ok.format(), PixelFormat::MonoBlack);
        assert_eq!(ok.color, ColorInfo::wbmp_default());
        assert!(ok.metadata.is_empty());
        // Padded last row is allowed: stride × (h − 1) + row.
        let padded = WbmpImage::packed(9, 2, PixelFormat::MonoBlack, 4, vec![0; 6]).unwrap();
        assert!(!padded.is_tightly_packed());
        assert_eq!(padded.wire_bits().len(), 4);
    }

    #[test]
    fn to_rgb8_and_to_rgba8_expand_bits_exactly_in_both_polarities() {
        // 3×1: white, black, white in wire polarity → 0b101_00000.
        let img = WbmpImage::from_bits(3, 1, vec![0b1010_0000]).unwrap();
        assert_eq!(img.to_gray8(), vec![255, 0, 255]);
        assert_eq!(img.to_rgb8(), vec![255, 255, 255, 0, 0, 0, 255, 255, 255]);
        assert_eq!(
            img.to_rgba8(),
            vec![255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
        assert!(img.is_white(0, 0) && !img.is_white(1, 0) && img.is_white(2, 0));
        assert!(!img.is_white(3, 0) && !img.is_white(0, 1));
        let inv = img.clone().into_format(PixelFormat::MonoWhite);
        assert_eq!(inv.format(), PixelFormat::MonoWhite);
        // Bits flipped, the five padding bits re-zeroed.
        assert_eq!(inv.as_bytes().unwrap(), &[0b0100_0000]);
        assert_eq!(inv.to_gray8(), img.to_gray8());
        assert_eq!(inv.wire_bits().as_ref(), img.as_bytes().unwrap());
        assert_eq!(inv.into_format(PixelFormat::MonoBlack), img);
    }

    #[test]
    fn wire_bits_borrows_canonical_native_planes_and_repacks_others() {
        let img = WbmpImage::from_bits(11, 2, vec![0xFF, 0xE0, 0x00, 0x20]).unwrap();
        assert!(matches!(img.wire_bits(), Cow::Borrowed(_)));
        // Dirty padding bits are masked in a copy.
        let dirty = WbmpImage::from_bits(11, 1, vec![0xFF, 0xFF]).unwrap();
        assert_eq!(dirty.wire_bits().as_ref(), &[0xFF, 0xE0]);
        // Padded stride is repacked.
        let padded =
            WbmpImage::packed(11, 2, PixelFormat::MonoBlack, 3, vec![1, 2, 9, 3, 4, 9]).unwrap();
        assert_eq!(padded.wire_bits().as_ref(), &[1, 0, 3, 0]);
    }

    #[test]
    fn from_rgb8_and_from_rgba8_quantise_luma_at_128() {
        // 2×1: pure white, pure black.
        let img = WbmpImage::from_rgb8(2, 1, vec![255, 255, 255, 0, 0, 0]).unwrap();
        assert_eq!(img.as_bytes().unwrap(), &[0b1000_0000]);
        // Mid-grey 127 → black, 128 → white; alpha ignored.
        let img = WbmpImage::from_rgba8(2, 1, vec![127, 127, 127, 0, 128, 128, 128, 0]).unwrap();
        assert_eq!(img.as_bytes().unwrap(), &[0b0100_0000]);
        // Pure green has luma 587/1000 → white; pure blue 114 → black.
        let img = WbmpImage::from_rgb8(2, 1, vec![0, 255, 0, 0, 0, 255]).unwrap();
        assert_eq!(img.as_bytes().unwrap(), &[0b1000_0000]);
        assert!(matches!(
            WbmpImage::from_rgb8(2, 1, vec![0; 5]),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            WbmpImage::from_rgba8(0, 1, vec![]),
            Err(WbmpError::InvalidData(_))
        ));
        assert_eq!(luma(255, 255, 255), 255);
        assert_eq!(luma(0, 0, 0), 0);
        assert_eq!(luma(255, 0, 0), 76);
    }
}
