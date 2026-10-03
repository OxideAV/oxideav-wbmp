//! WBMP Type-0 decoder.
//!
//! Parses the header then copies the packed 1-bit-per-pixel plane
//! verbatim — the on-disk byte layout already matches
//! [`WbmpPixelFormat::MonoBlack`] (MSB-first, 1 = white) so no
//! per-pixel transform is required unless the caller asked for the
//! inverse polarity ([`crate::DecodeOptions::format`]).
//!
//! The contract entry points live at the crate root ([`crate::decode`],
//! [`crate::decode_with`], [`crate::decode_all`], …) and call the
//! crate-internal `decode_image` / `decode_frames` here. The
//! pre-contract `parse_wbmp*` family remains as `#[deprecated]`
//! wrappers for one release. With the default `registry` feature on,
//! the gated `WbmpDecoder` trait impl wraps the same functions for the
//! `oxideav_core::Decoder` surface.

use crate::error::{Result, WbmpError};
use crate::ext::ExtFields;
use crate::header::{parse_header_ext_inner, parse_header_inner, ParsedHeader};
use crate::image::{Frame, PlaneLayout, WbmpImage, WbmpPixelFormat};
#[allow(deprecated)]
use crate::image::{Plane, WbmpPlane};
#[allow(deprecated)]
use crate::limits::WbmpLimits;
use crate::options::DecodeOptions;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Packet, PixelFormat, VideoFrame};

/// Maximum number of animated sub-images that may follow the main
/// image in a WBMP stream (WAP-237 §4.2, §4.5.1: "The WBMP image can
/// have at most 15 animated images following the main image").
///
/// The total frame count returned by [`crate::decode_all`] is therefore
/// at most `1 + MAX_ANIMATED_IMAGES == 16` (the main image plus up to 15
/// animated sub-images).
pub const MAX_ANIMATED_IMAGES: usize = 15;

// ---------------------------------------------------------------------------
// Crate-internal implementation (the one the root API and the registry
// adapter share).
// ---------------------------------------------------------------------------

/// Parse the header the way `opts.strict` asks for: lenient = the
/// general-form §4.4.1 parse (extension headers honoured and
/// surfaced); strict = Type-0 conformance (`FixHeaderField == 0x00`,
/// shortest MBIs).
pub(crate) fn parse_header_for(bytes: &[u8], strict: bool) -> Result<ParsedHeader> {
    if strict {
        parse_header_inner(bytes, true)
    } else {
        parse_header_ext_inner(bytes, false)
    }
}

/// Decode the main image per `opts`.
pub(crate) fn decode_image(bytes: &[u8], opts: &DecodeOptions) -> Result<WbmpImage> {
    let header = parse_header_for(bytes, opts.strict)?;
    decode_body(bytes, &header, opts)
}

/// Decode the main image plus every animated sub-image per `opts`.
pub(crate) fn decode_frames(bytes: &[u8], opts: &DecodeOptions) -> Result<Vec<Frame>> {
    let header = parse_header_for(bytes, opts.strict)?;
    // The main image goes through decode_body, which applies every
    // limit check (dimensions, pixel-byte cap, overflow guard) and the
    // truncation check, then copies the first plane verbatim.
    let main = decode_body(bytes, &header, opts)?;
    let layout = PlaneLayout::new(header.width, header.height)
        .map_err(|msg| WbmpError::invalid(msg.to_string()))?;
    let extra = animated_frame_count(bytes.len(), header.data_offset, layout.total_bytes);

    let mut frames = Vec::with_capacity(1 + extra);
    frames.push(Frame::new(main, 0));
    let mut offset = header.data_offset + layout.total_bytes;
    for i in 0..extra {
        let end = offset + layout.total_bytes;
        let mut img =
            WbmpImage::from_bits(header.width, header.height, bytes[offset..end].to_vec())?;
        img.convert_format(opts.format);
        frames.push(Frame::new(img, (i + 1) as u32));
        offset = end;
    }
    Ok(frames)
}

/// How many whole animated sub-images follow the main image in a
/// buffer of `input_len` bytes: `min(15, (input_len - data_offset -
/// total_bytes) / total_bytes)`, `0` when the main image itself does
/// not fit. A trailing run shorter than one frame is ignorable padding
/// (same posture as the single-image decode toward trailing bytes).
pub(crate) fn animated_frame_count(
    input_len: usize,
    data_offset: usize,
    total_bytes: usize,
) -> usize {
    if total_bytes == 0 {
        return 0;
    }
    let after_main = input_len.saturating_sub(data_offset.saturating_add(total_bytes));
    (after_main / total_bytes).min(MAX_ANIMATED_IMAGES)
}

/// Decode the main image data given an already-parsed header: limit
/// checks (before any allocation), the truncation check, the verbatim
/// row copy and the optional polarity flip.
fn decode_body(input: &[u8], header: &ParsedHeader, opts: &DecodeOptions) -> Result<WbmpImage> {
    let (width, height) = (header.width, header.height);
    let layout =
        PlaneLayout::new(width, height).map_err(|msg| WbmpError::invalid(msg.to_string()))?;
    opts.check(width, height, layout.total_bytes as u64)?;

    let body = input.get(header.data_offset..).unwrap_or(&[]);
    if body.len() < layout.total_bytes {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel data truncated (need {} bytes, got {})",
            layout.total_bytes,
            body.len()
        )));
    }

    // Byte layout matches the native plane format directly — copy
    // verbatim. Trailing bytes past `layout.total_bytes` (animated
    // sub-images, or padding) are left to `decode_frames`.
    let mut image = WbmpImage::from_bits(width, height, body[..layout.total_bytes].to_vec())?;
    image.convert_format(opts.format);
    Ok(image)
}

/// `WbmpLimits`-era decode: the opaque-`FixHeaderField` Type-0 header
/// parse (lax or strict) followed by the shared body decode.
#[allow(deprecated)]
fn legacy_decode(input: &[u8], limits: &WbmpLimits, strict: bool) -> Result<WbmpImage> {
    let header = parse_header_inner(input, strict)?;
    decode_body(input, &header, &DecodeOptions::from(*limits))
}

// ---------------------------------------------------------------------------
// Registry-side Decoder trait surface.
// ---------------------------------------------------------------------------

/// Factory registered with the codec registry. One packet per whole
/// WBMP file; one frame (the main image) per packet.
///
/// The frame carries the wire polarity
/// ([`oxideav_core::PixelFormat::MonoBlack`], 1 = white) unless the
/// caller set `params.pixel_format = Some(PixelFormat::MonoWhite)`, in
/// which case the polarity flip + padding-bit mask happen in-place
/// during decode. Any other value (including `None`) keeps `MonoBlack`.
#[cfg(feature = "registry")]
pub fn make_decoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    let target = match params.pixel_format {
        Some(PixelFormat::MonoWhite) => WbmpPixelFormat::MonoWhite,
        _ => WbmpPixelFormat::MonoBlack,
    };
    Ok(Box::new(WbmpDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
        opts: DecodeOptions::default().with_format(target),
    }))
}

#[cfg(feature = "registry")]
struct WbmpDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
    opts: DecodeOptions,
}

#[cfg(feature = "registry")]
impl Decoder for WbmpDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = decode_image(&packet.data, &self.opts)?;
        self.pending = Some(crate::registry::image_into_video_frame(image, packet.pts));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<oxideav_core::Frame> {
        match self.pending.take() {
            Some(f) => Ok(oxideav_core::Frame::Video(f)),
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Deprecated pre-contract entry points (one release).
// ---------------------------------------------------------------------------

/// A decoded WBMP stream including any animated sub-images that follow
/// the main image (WAP-237 §4.2 / §4.5.1) — the pre-contract shape of
/// [`crate::decode_all`].
#[deprecated(note = "use oxideav_wbmp::decode_all -> Vec<Frame> (IMAGE_CRATE_API)")]
#[derive(Debug, Clone)]
pub struct WbmpAnimation {
    /// Picture width in pixels (shared by every frame).
    pub width: u32,
    /// Picture height in pixels (shared by every frame).
    pub height: u32,
    /// Pixel layout the planes carry — always [`WbmpPixelFormat::MonoBlack`]
    /// from this entry point (the on-disk polarity).
    pub pixel_format: WbmpPixelFormat,
    /// One packed plane per frame: index 0 is the main image, indices
    /// `1..` are the animated sub-images in stream order. Always at
    /// least one element; at most `1 + MAX_ANIMATED_IMAGES`.
    pub frames: Vec<Plane>,
}

#[allow(deprecated)]
impl WbmpAnimation {
    /// Number of animated sub-images following the main image (i.e.
    /// `frames.len() - 1`). `0` for a single-frame WBMP.
    pub fn animated_count(&self) -> usize {
        self.frames.len() - 1
    }

    /// `true` when the stream carries at least one animated sub-image.
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// View the main image (frame 0) as a standalone [`WbmpImage`],
    /// discarding any animated sub-images.
    pub fn main_image(&self) -> WbmpImage {
        WbmpImage::packed(
            self.width,
            self.height,
            self.pixel_format,
            self.frames[0].stride,
            self.frames[0].data.clone(),
        )
        .expect("WbmpAnimation frames are decoder-validated")
    }
}

/// Decode a complete WBMP file (Type 0 only) into a [`WbmpImage`]
/// using the pre-contract default [`WbmpLimits`] (16384 × 16384,
/// 8 MiB) and the opaque-`FixHeaderField` header parse.
#[deprecated(note = "use oxideav_wbmp::decode (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp(input: &[u8]) -> Result<WbmpImage> {
    legacy_decode(input, &WbmpLimits::default(), false)
}

/// [`parse_wbmp`] with caller-supplied [`WbmpLimits`].
#[deprecated(note = "use oxideav_wbmp::decode_with(.., &DecodeOptions) (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    legacy_decode(input, limits, false)
}

/// Strict variant of [`parse_wbmp`]: the `FixedHeader` byte must be
/// exactly `0x00` and every MBI must be in shortest form.
#[deprecated(
    note = "use oxideav_wbmp::decode_with(.., &DecodeOptions::new().with_strict(true)) (IMAGE_CRATE_API)"
)]
#[allow(deprecated)]
pub fn parse_wbmp_strict(input: &[u8]) -> Result<WbmpImage> {
    legacy_decode(input, &WbmpLimits::default(), true)
}

/// Strict variant of [`parse_wbmp_with_limits`].
#[deprecated(
    note = "use oxideav_wbmp::decode_with(.., &DecodeOptions::new().with_strict(true)) (IMAGE_CRATE_API)"
)]
#[allow(deprecated)]
pub fn parse_wbmp_strict_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    legacy_decode(input, limits, true)
}

/// Decoded WBMP image paired with any parsed extension headers — the
/// pre-contract shape of [`crate::decode`] + [`crate::info`]
/// (`ImageInfo::ext_fields`).
#[deprecated(note = "use oxideav_wbmp::decode + oxideav_wbmp::info (IMAGE_CRATE_API)")]
#[derive(Debug, Clone)]
pub struct WbmpImageExt {
    /// The decoded main image.
    pub image: WbmpImage,
    /// The parsed `ExtFields` region, or `None` when the
    /// `FixHeaderField` presence flag was clear.
    pub ext_fields: Option<ExtFields>,
}

/// Extension-header-aware decode with the pre-contract default
/// [`WbmpLimits`] — what [`crate::decode`] now does by default.
#[deprecated(note = "use oxideav_wbmp::decode + oxideav_wbmp::info (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp_ext(input: &[u8]) -> Result<WbmpImageExt> {
    parse_wbmp_ext_with_limits(input, &WbmpLimits::default())
}

/// Extension-header-aware decode with caller-supplied [`WbmpLimits`].
#[deprecated(note = "use oxideav_wbmp::decode_with + oxideav_wbmp::info (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp_ext_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImageExt> {
    let header = parse_header_ext_inner(input, false)?;
    let image = decode_body(input, &header, &DecodeOptions::from(*limits))?;
    Ok(WbmpImageExt {
        image,
        ext_fields: header.ext_fields,
    })
}

/// Decode a WBMP stream into its main image and any animated
/// sub-images with the pre-contract default [`WbmpLimits`].
#[deprecated(note = "use oxideav_wbmp::decode_all (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp_frames(input: &[u8]) -> Result<WbmpAnimation> {
    parse_wbmp_frames_with_limits(input, &WbmpLimits::default())
}

/// Animated-aware decode with caller-supplied [`WbmpLimits`].
#[deprecated(note = "use oxideav_wbmp::decode_all_with (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn parse_wbmp_frames_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpAnimation> {
    let header = parse_header_inner(input, false)?;
    let opts = DecodeOptions::from(*limits);
    let main = decode_body(input, &header, &opts)?;
    let layout = PlaneLayout::new(header.width, header.height)
        .map_err(|msg| WbmpError::invalid(msg.to_string()))?;
    let extra = animated_frame_count(input.len(), header.data_offset, layout.total_bytes);
    let mut frames: Vec<WbmpPlane> = Vec::with_capacity(1 + extra);
    frames.push(main.planes.into_iter().next().expect("main image plane"));
    let mut offset = header.data_offset + layout.total_bytes;
    for _ in 0..extra {
        let end = offset + layout.total_bytes;
        frames.push(Plane::new(layout.stride, input[offset..end].to_vec()));
        offset = end;
    }
    Ok(WbmpAnimation {
        width: header.width,
        height: header.height,
        pixel_format: WbmpPixelFormat::MonoBlack,
        frames,
    })
}

/// Decode a WBMP file into the requested polarity with the
/// pre-contract default [`WbmpLimits`].
#[deprecated(
    note = "use oxideav_wbmp::decode_with(.., &DecodeOptions::new().with_format(..)) (IMAGE_CRATE_API)"
)]
#[allow(deprecated)]
pub fn parse_wbmp_as(input: &[u8], target: WbmpPixelFormat) -> Result<WbmpImage> {
    parse_wbmp_as_with_limits(input, target, &WbmpLimits::default())
}

/// Decode a WBMP file into the requested polarity with caller-supplied
/// [`WbmpLimits`].
#[deprecated(
    note = "use oxideav_wbmp::decode_with(.., &DecodeOptions::new().with_format(..)) (IMAGE_CRATE_API)"
)]
#[allow(deprecated)]
pub fn parse_wbmp_as_with_limits(
    input: &[u8],
    target: WbmpPixelFormat,
    limits: &WbmpLimits,
) -> Result<WbmpImage> {
    let mut image = legacy_decode(input, limits, false)?;
    image.convert_format(target);
    Ok(image)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::header::write_header;
    use crate::mbi::write_mbi_u32;

    #[test]
    fn parse_minimal_1x1_white() {
        // 1×1 image, single pixel = white (bit 7 of byte 0 set).
        let mut buf = Vec::new();
        write_header(1, 1, &mut buf);
        buf.push(0b1000_0000);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.format, WbmpPixelFormat::MonoBlack);
        assert_eq!(image.planes.len(), 1);
        assert_eq!(image.planes[0].stride, 1);
        assert_eq!(image.planes[0].data, [0b1000_0000]);
    }

    #[test]
    fn parse_padded_row() {
        // 11×1: row needs 2 bytes (16 bits, last 5 padding).
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.planes[0].stride, 2);
        assert_eq!(image.planes[0].data, [0b1010_1100, 0b1110_0000]);
    }

    #[test]
    fn parse_truncated_pixel_data_errors() {
        // Header says 16×1 (2 bytes per row) but only 1 body byte
        // present.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf);
        buf.push(0xFF);
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn parse_rejects_unknown_type() {
        // Type=1 — not standardised.
        let buf = [
            0x01u8, 0x00, 0x08, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)));
    }

    #[test]
    fn trailing_bytes_after_pixel_data_are_ignored() {
        // 8×1 (1 body byte) + 3 garbage bytes afterwards.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0x55);
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE]);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.planes[0].data, [0x55]);
    }

    // --- Hardening tests against malformed / adversarial input. ---

    #[test]
    fn rejects_oversized_width_under_default_limits() {
        // Width MBI = 0x82_80_00 (= 32768) — twice the default
        // max_width of 16384. Decoder must error before touching the
        // allocator.
        let buf = [
            0x00u8, 0x00, // Type=0, FixedHeader
            0x82, 0x80, 0x00, // Width = 32768
            0x01, // Height = 1
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn rejects_oversized_height_under_default_limits() {
        // Width=1, Height=32768.
        let buf = [
            0x00u8, 0x00, // Type=0, FixedHeader
            0x01, // Width = 1
            0x82, 0x80, 0x00, // Height = 32768
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn rejects_pixel_byte_blowup_under_default_limits() {
        // 16000 × 16000 sneaks under max_width/max_height (both 16384)
        // but width*height/8 = 32 MB blows past max_pixel_bytes
        // (8 MiB).
        let mut buf = Vec::new();
        write_header(16000, 16000, &mut buf);
        // Don't append any pixel data — limit check fires first.
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn unbounded_limits_admit_larger_image() {
        // A 20000 × 1 (only 2500 body bytes) blows the default width
        // cap but is fine with WbmpLimits::unbounded() and the cheap
        // body.
        let mut buf = Vec::new();
        write_header(20000, 1, &mut buf);
        buf.extend_from_slice(&[0u8; 2500]);
        assert!(matches!(
            parse_wbmp(&buf).unwrap_err(),
            WbmpError::LimitExceeded(_)
        ));
        let img = parse_wbmp_with_limits(&buf, &WbmpLimits::unbounded()).unwrap();
        assert_eq!(img.width, 20000);
        assert_eq!(img.height, 1);
        assert_eq!(img.planes[0].data.len(), 2500);
    }

    #[test]
    fn custom_limits_can_be_tighter_than_defaults() {
        // Caller wants max 64-pixel images. A 65×1 must be rejected.
        let mut buf = Vec::new();
        write_header(65, 1, &mut buf);
        buf.push(0u8);
        buf.push(0u8);
        let tight = WbmpLimits {
            max_width: 64,
            ..WbmpLimits::default()
        };
        let err = parse_wbmp_with_limits(&buf, &tight).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn fuzz_short_byte_prefixes_never_panic() {
        // Every 1-byte and most 2-byte sequences either parse (small
        // images) or return a tidy error; none should panic. We feed
        // every possible 2-byte prefix, then enough random-ish
        // 3..=8-byte sequences to cover all error paths in the header
        // parser plus the body-length check.
        for a in 0u8..=255 {
            for b in 0u8..=255 {
                let _ = parse_wbmp(&[a, b]);
            }
        }
        // Random-ish coverage of slightly longer inputs — fixed seed
        // (LCG) so the test is deterministic.
        let mut seed: u64 = 0xDEAD_BEEF_CAFE_BABE;
        for _ in 0..4096 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = 1 + (seed as usize) % 20;
            let mut buf = vec![0u8; len];
            for byte in buf.iter_mut() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = seed as u8;
            }
            let _ = parse_wbmp(&buf);
        }
    }

    // --- Polarity (MonoBlack ↔ MonoWhite) decode tests. ---

    #[test]
    fn parse_as_monowhite_matches_parse_wbmp() {
        // parse_wbmp_as(MonoBlack) must be byte-for-byte identical to
        // parse_wbmp for the same input.
        let mut buf = Vec::new();
        write_header(11, 3, &mut buf);
        let body = [0xAA, 0x80, 0x55, 0xA0, 0xC3, 0x40];
        buf.extend_from_slice(&body);
        let a = parse_wbmp(&buf).unwrap();
        let b = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap();
        assert_eq!(a.format, WbmpPixelFormat::MonoBlack);
        assert_eq!(b.format, WbmpPixelFormat::MonoBlack);
        assert_eq!(a.planes[0].data, b.planes[0].data);
    }

    #[test]
    fn parse_as_monoblack_inverts_full_byte_rows() {
        // 8×1 byte-aligned row: the polarity flip is a clean `!byte`
        // with no padding to mask.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0b1010_0110);
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoWhite).unwrap();
        assert_eq!(img.format, WbmpPixelFormat::MonoWhite);
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, [0b0101_1001]);
    }

    #[test]
    fn parse_as_monoblack_masks_padding_bits() {
        // 11×1 → stride 2, 5 padding bits in the last byte. After
        // inversion those would become five trailing `1` bits unless
        // masked. We assert they're back to zero.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        // First 11 bits (MSB-first): 1010 1100 111 — packed
        // 0xAC, then 0xE0 (with 5 padding zeros).
        buf.push(0xAC);
        buf.push(0xE0);
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoWhite).unwrap();
        // Inverted: 0x53 in byte 0; byte 1 inversion would give
        // 0x1F, but masking the 5 padding bits zeroes them → 0x00.
        assert_eq!(img.planes[0].data, [0x53, 0x00]);
    }

    #[test]
    fn parse_as_monoblack_roundtrips_through_inversion() {
        // Decoding to MonoWhite twice (via parse_as → encode → parse_as)
        // recovers the original on-disk bits exactly. We pick a width
        // with a non-trivial padding tail (159 → 4 padding bits).
        let stride = WbmpImage::row_stride(159);
        let mut bits = vec![0u8; stride * 5];
        let mut seed: u64 = 0xC0FF_EE00_BAAD_F00D;
        for byte in bits.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = seed as u8;
        }
        // Pre-zero the padding bits in the test fixture so the round
        // trip target is well-formed input → MonoBlack verbatim.
        let pad_bits = stride * 8 - 159;
        let mask: u8 = 0xFFu8 << pad_bits;
        for y in 0..5 {
            let last = y * stride + (stride - 1);
            bits[last] &= mask;
        }
        let encoded = crate::encoder::encode_wbmp(159, 5, &bits).unwrap();
        // First decode: MonoWhite (inverted with padding masked).
        let blk = parse_wbmp_as(&encoded, WbmpPixelFormat::MonoWhite).unwrap();
        assert_eq!(blk.format, WbmpPixelFormat::MonoWhite);
        // Manually invert + mask: must match the MonoWhite plane bit-
        // for-bit.
        let mut expect = bits.clone();
        for b in expect.iter_mut() {
            *b = !*b;
        }
        for y in 0..5 {
            let last = y * stride + (stride - 1);
            expect[last] &= mask;
        }
        assert_eq!(blk.planes[0].data, expect);
    }

    #[test]
    fn parse_as_monoblack_respects_limits() {
        // Limit checks fire before the polarity flip — a MonoWhite
        // decode of an over-sized header must still raise
        // LimitExceeded, not run through the inversion loop on
        // unallocated memory.
        let mut buf = Vec::new();
        write_header(16_000, 16_000, &mut buf);
        let err = parse_wbmp_as(&buf, WbmpPixelFormat::MonoWhite).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_as_with_limits_propagates_unbounded() {
        // 20000×1 (2500 body bytes) blows the default width cap but
        // passes with WbmpLimits::unbounded(), in both polarities.
        let mut buf = Vec::new();
        write_header(20_000, 1, &mut buf);
        buf.extend_from_slice(&[0xFFu8; 2500]);
        let lim = WbmpLimits::unbounded();
        let w = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoBlack, &lim).unwrap();
        let b = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoWhite, &lim).unwrap();
        assert_eq!(w.planes[0].data, vec![0xFFu8; 2500]);
        assert_eq!(b.planes[0].data, vec![0x00u8; 2500]);
    }

    // --- Strict-mode reject path (FixedHeader == 0x00 required). ---

    #[test]
    fn parse_wbmp_strict_matches_lax_on_conformant_input() {
        // Well-formed Type-0 file (FixedHeader = 0x00): the strict and
        // lax entry points must produce byte-for-byte identical
        // results.
        let mut buf = Vec::new();
        write_header(11, 2, &mut buf);
        let body = [0xAC, 0xE0, 0x53, 0x00];
        buf.extend_from_slice(&body);
        let lax = parse_wbmp(&buf).unwrap();
        let strict = parse_wbmp_strict(&buf).unwrap();
        assert_eq!(lax.width, strict.width);
        assert_eq!(lax.height, strict.height);
        assert_eq!(lax.format, strict.format);
        assert_eq!(lax.planes[0].stride, strict.planes[0].stride);
        assert_eq!(lax.planes[0].data, strict.planes[0].data);
    }

    #[test]
    fn parse_wbmp_strict_rejects_nonzero_fixed_header() {
        // Same bytes as parse_padded_row but with FixedHeader = 0xFF.
        // The lax parser still accepts it (forward-compat); the strict
        // parser must error out as InvalidData.
        let buf = [
            0x00u8, // Type = 0
            0xFF,   // FixedHeader = 0xFF (non-conformant)
            0x0B,   // Width = 11
            0x01,   // Height = 1
            0xAC, 0xE0, // 11 pixels packed (5 bits padding)
        ];
        assert!(parse_wbmp(&buf).is_ok(), "lax parser still accepts");
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_rejects_redundantly_padded_dimension_mbi() {
        // §4.3.1 shortest-encoding MUST NOT, now enforced on the full
        // strict decode path: a leading-0x80-padded Width MBI is decoded
        // fine by the lax parser but rejected by the strict one. The
        // FixedHeader is the conformant 0x00 so this isolates the MBI
        // shortest-encoding check from the FixedHeader check.
        let buf = [
            0x00u8, // Type = 0
            0x00,   // FixedHeader = 0x00 (conformant)
            0x80, 0x0B, // Width = 11, but non-minimal (leading 0x80)
            0x01, // Height = 1
            0xAC, 0xE0, // 11 pixels packed
        ];
        let lax = parse_wbmp(&buf).unwrap();
        assert_eq!(lax.width, 11);
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_with_limits_enforces_both() {
        // FixedHeader violation fires first (before the limit check)
        // when the file is also out of bounds — the strict header path
        // runs before allocation.
        let buf = [
            0x00u8, 0x01, // FixedHeader = 0x01 — strict rejects
            0x82, 0x80, 0x00, // Width = 32768 (would also be over the default cap)
            0x01, // Height = 1
        ];
        let err = parse_wbmp_strict_with_limits(&buf, &WbmpLimits::default()).unwrap_err();
        // We promise InvalidData here, not LimitExceeded — strict mode
        // wants to surface the FixedHeader violation as soon as it
        // sees it, before the limit machinery.
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_still_enforces_limits_on_conformant_header() {
        // FixedHeader = 0x00 (conformant) but dimensions blow the
        // default limit. Strict mode must still return LimitExceeded
        // — strict is an ADDITIONAL check, not a replacement.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_still_rejects_nonzero_type() {
        // Non-zero Type field must surface as Unsupported in both
        // parsers; strict mode tightens the FixedHeader check, not the
        // Type check.
        let buf = [0x01u8, 0x00, 0x08, 0x08];
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    // --- parse_wbmp_ext (extension-header-aware decode) tests. ---

    #[test]
    fn parse_ext_conformant_type0_matches_plain_decode() {
        // FixHeaderField = 0x00 → no ExtFields. The ext-aware decode
        // must produce the identical image and report ext_fields None.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let plain = parse_wbmp(&buf).unwrap();
        let ext = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(ext.image.width, plain.width);
        assert_eq!(ext.image.height, plain.height);
        assert_eq!(ext.image.format, plain.format);
        assert_eq!(ext.image.planes[0].stride, plain.planes[0].stride);
        assert_eq!(ext.image.planes[0].data, plain.planes[0].data);
        assert!(ext.ext_fields.is_none());
    }

    #[test]
    fn parse_ext_decodes_image_after_parameter_pairs() {
        // Non-conformant Type-0 file carrying a Type-11 ExtFields region
        // before the dimensions. parse_wbmp would mis-read the
        // ParameterHeader octet as the Width MBI; parse_wbmp_ext must
        // skip the ExtFields and decode the real 8x1 image.
        use crate::ext::{write_ext_fields, ExtFields, Parameter};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf); // Type = 0
        buf.push(0b1110_0000); // FixHeaderField: ext follow, type 11
        let ext = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"id".to_vec(),
            value: b"v".to_vec(),
        }]);
        write_ext_fields(&ext, &mut buf).unwrap();
        write_mbi_u32(8, &mut buf); // Width = 8
        write_mbi_u32(1, &mut buf); // Height = 1
        buf.push(0b1010_1010); // one body byte (8x1 = 1 byte/row)

        let parsed = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(parsed.image.width, 8);
        assert_eq!(parsed.image.height, 1);
        assert_eq!(parsed.image.planes[0].data, [0b1010_1010]);
        assert_eq!(parsed.image.format, WbmpPixelFormat::MonoBlack);
        assert_eq!(parsed.ext_fields, Some(ext));
    }

    #[test]
    fn parse_ext_decodes_image_after_bitfield00_chain() {
        use crate::ext::{write_ext_fields, ExtFields};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf); // Type = 0
        buf.push(0b1000_0000); // FixHeaderField: ext follow, type 00
        let ext = ExtFields::Bitfield00(vec![0x01, 0x42]);
        write_ext_fields(&ext, &mut buf).unwrap();
        write_mbi_u32(4, &mut buf); // Width = 4
        write_mbi_u32(2, &mut buf); // Height = 2
        buf.push(0b1100_0000); // row 0 (4px in 1 byte)
        buf.push(0b0011_0000); // row 1

        let parsed = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(parsed.image.width, 4);
        assert_eq!(parsed.image.height, 2);
        assert_eq!(parsed.image.planes[0].data, [0b1100_0000, 0b0011_0000]);
        assert_eq!(parsed.ext_fields, Some(ext));
    }

    #[test]
    fn parse_ext_enforces_limits() {
        // Conformant header but dimensions blow the default cap — the
        // ext-aware path must still apply WbmpLimits via decode_body.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_ext_truncated_pixel_data_errors() {
        // ExtFields parse fine, dimensions fine, but the body is short.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf); // needs 2 body bytes
        buf.push(0x00); // only 1
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_ext_rejects_truncated_ext_region() {
        // FixHeaderField says ExtFields follow (type 00) but the
        // continuation bit is set with no terminating octet → the body
        // never starts. Must error, not panic.
        let buf = [
            0x00u8,      // Type = 0
            0b1000_0000, // FixHeaderField: ext follow, type 00
            0x80,        // bitfield octet with continuation bit, stream ends
        ];
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    // --- Animated sub-image decode tests (WAP-237 §4.2 / §4.5.1). ---

    #[test]
    fn frames_single_image_matches_parse_wbmp() {
        // A conformant single-frame WBMP yields exactly one frame, plane
        // byte-identical to parse_wbmp.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let plain = parse_wbmp(&buf).unwrap();
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 1);
        assert!(!anim.is_animated());
        assert_eq!(anim.animated_count(), 0);
        assert_eq!(anim.width, plain.width);
        assert_eq!(anim.height, plain.height);
        assert_eq!(anim.pixel_format, plain.format);
        assert_eq!(anim.frames[0].stride, plain.planes[0].stride);
        assert_eq!(anim.frames[0].data, plain.planes[0].data);
        // main_image() reproduces the single-frame view.
        let main = anim.main_image();
        assert_eq!(main.planes[0].data, plain.planes[0].data);
        assert_eq!(main.width, plain.width);
    }

    #[test]
    fn frames_decodes_main_plus_animated_subimages() {
        // 8×1 main image + two animated sub-images, each 1 body byte.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0b1111_0000); // main
        buf.push(0b0000_1111); // animated frame 1
        buf.push(0b1010_1010); // animated frame 2
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 3);
        assert!(anim.is_animated());
        assert_eq!(anim.animated_count(), 2);
        assert_eq!(anim.frames[0].data, [0b1111_0000]);
        assert_eq!(anim.frames[1].data, [0b0000_1111]);
        assert_eq!(anim.frames[2].data, [0b1010_1010]);
        // Frame 0 still matches the single-frame parse_wbmp (which
        // ignores the trailing animated bytes).
        let plain = parse_wbmp(&buf).unwrap();
        assert_eq!(anim.frames[0].data, plain.planes[0].data);
    }

    #[test]
    fn frames_multibyte_rows_animated() {
        // 11×2 → stride 2, total_bytes 4 per frame. Main + 1 animated.
        let mut buf = Vec::new();
        write_header(11, 2, &mut buf);
        let main = [0xAC, 0xE0, 0x53, 0x00];
        let f1 = [0x12, 0x80, 0x34, 0x40];
        buf.extend_from_slice(&main);
        buf.extend_from_slice(&f1);
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].stride, 2);
        assert_eq!(anim.frames[0].data, main);
        assert_eq!(anim.frames[1].data, f1);
    }

    #[test]
    fn frames_partial_trailing_run_is_ignored() {
        // 8×1 main + 2 full animated frames + 0 stray bytes that don't
        // make a full frame (stride*height = 1 here, so use a 11×1 image
        // where a single stray byte is < the 2-byte frame size).
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf); // stride 2, frame = 2 bytes
        buf.extend_from_slice(&[0xAC, 0xE0]); // main
        buf.extend_from_slice(&[0x12, 0x80]); // animated frame 1
        buf.push(0x77); // a single stray byte — < one full frame
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].data, [0xAC, 0xE0]);
        assert_eq!(anim.frames[1].data, [0x12, 0x80]);
    }

    #[test]
    fn frames_caps_at_max_animated_images() {
        // 8×1 main + 20 animated-frame-sized chunks. The §4.5.1 cap is
        // 15 animated images, so the decoder must stop after 16 total
        // frames and ignore the rest.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        for i in 0..=20u8 {
            buf.push(i);
        }
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 1 + MAX_ANIMATED_IMAGES);
        assert_eq!(anim.animated_count(), MAX_ANIMATED_IMAGES);
        // Frames 0..=15 carry bytes 0..=15; bytes 16..=20 are dropped.
        for (i, frame) in anim.frames.iter().enumerate() {
            assert_eq!(frame.data, [i as u8]);
        }
    }

    #[test]
    fn frames_truncated_main_image_errors() {
        // The main image itself is short — must surface InvalidData, not
        // silently return zero frames.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf); // needs 2 body bytes
        buf.push(0xFF); // only 1
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn frames_rejects_non_zero_type() {
        let buf = [0x01u8, 0x00, 0x08, 0x08, 0xFF];
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn frames_enforces_per_frame_limits() {
        // Dimensions blow the default cap — the per-frame limit check in
        // decode_body must fire before any frame is allocated.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn frames_with_unbounded_limits_decodes_large_main() {
        // 20000×1 (2500 bytes/frame) blows the default width cap but is
        // fine with unbounded limits; one main + one animated frame.
        let mut buf = Vec::new();
        write_header(20_000, 1, &mut buf);
        buf.extend_from_slice(&[0xAAu8; 2500]); // main
        buf.extend_from_slice(&[0x55u8; 2500]); // animated frame 1
        assert!(matches!(
            parse_wbmp_frames(&buf).unwrap_err(),
            WbmpError::LimitExceeded(_)
        ));
        let anim = parse_wbmp_frames_with_limits(&buf, &WbmpLimits::unbounded()).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].data, vec![0xAAu8; 2500]);
        assert_eq!(anim.frames[1].data, vec![0x55u8; 2500]);
    }

    #[test]
    fn fuzz_frames_never_panic() {
        // Adversarial inputs to the animated-frame path must return a
        // Result, never panic / over-read / OOM.
        for a in 0u8..=255 {
            for b in 0u8..=255 {
                let _ = parse_wbmp_frames(&[a, b]);
            }
        }
        let mut seed: u64 = 0x0BAD_F00D_DEAD_C0DE;
        for _ in 0..4096 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = 1 + (seed as usize) % 40;
            let mut buf = vec![0u8; len];
            for byte in buf.iter_mut() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = seed as u8;
            }
            let _ = parse_wbmp_frames(&buf);
        }
    }

    #[test]
    fn fuzz_padded_mbi_runs_never_panic() {
        // Adversarial header: Type=0, FixedHeader=0, then a long run
        // of continuation-bit-set bytes for both width and height. The
        // MBI cap should clamp without panicking or allocating.
        for run_len in 1..=12 {
            let mut buf = vec![0x00u8, 0x00];
            buf.extend(std::iter::repeat_n(0x80u8, run_len));
            buf.push(0x01); // closing byte
            buf.push(0x01); // height = 1
            buf.push(0x00); // 1 body byte
            let _ = parse_wbmp(&buf);
        }
    }
}
