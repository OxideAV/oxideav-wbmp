//! WBMP Type-0 decoder.
//!
//! Parses the [`crate::header::Header`] then copies the packed
//! 1-bit-per-pixel plane verbatim — the on-disk byte layout already
//! matches the [`WbmpPixelFormat::MonoWhite`] convention
//! (MSB-first, 1 = white) so no per-pixel transform is required.
//!
//! With the default `registry` feature on, the gated `WbmpDecoder` trait
//! impl wraps [`parse_wbmp`] for the `oxideav_core::Decoder` surface.

use crate::error::{Result, WbmpError};
use crate::header::{parse_header, parse_header_strict};
use crate::image::{PlaneLayout, WbmpImage, WbmpPixelFormat, WbmpPlane};
use crate::limits::WbmpLimits;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. One packet per whole
/// WBMP file; one frame per packet.
///
/// The caller can opt into [`oxideav_core::PixelFormat::MonoBlack`]
/// by setting `params.pixel_format = Some(PixelFormat::MonoBlack)` on
/// the [`CodecParameters`] passed into the registry — the decoder
/// will perform the polarity flip + padding-bit mask in-place during
/// decode. Any other value (including `None`) keeps the on-disk
/// [`PixelFormat::MonoWhite`] polarity.
#[cfg(feature = "registry")]
pub fn make_decoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    let target = match params.pixel_format {
        Some(PixelFormat::MonoBlack) => WbmpPixelFormat::MonoBlack,
        _ => WbmpPixelFormat::MonoWhite,
    };
    Ok(Box::new(WbmpDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
        target,
    }))
}

#[cfg(feature = "registry")]
struct WbmpDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
    target: WbmpPixelFormat,
}

#[cfg(feature = "registry")]
impl Decoder for WbmpDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_wbmp_as(&packet.data, self.target)?;
        self.pending = Some(image_to_video_frame(image));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
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

#[cfg(feature = "registry")]
fn image_to_video_frame(image: WbmpImage) -> VideoFrame {
    VideoFrame {
        pts: image.pts,
        planes: image
            .planes
            .into_iter()
            .map(|p| VideoPlane {
                stride: p.stride,
                data: p.data,
            })
            .collect(),
    }
}

/// Decode a complete WBMP file (Type 0 only) into a [`WbmpImage`]
/// using the default [`WbmpLimits`].
///
/// Returns:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero (no
///   other type is defined by WAP-237 normatively or widely
///   deployed).
/// * [`WbmpError::InvalidData`] for truncated headers, MBI overflow,
///   or pixel-data shorter than what `width × height` requires.
/// * [`WbmpError::LimitExceeded`] if the header dimensions or
///   computed pixel-data size exceeds the default
///   [`WbmpLimits`] — see [`parse_wbmp_with_limits`] for callers
///   that need to allow larger images.
pub fn parse_wbmp(input: &[u8]) -> Result<WbmpImage> {
    parse_wbmp_with_limits(input, &WbmpLimits::default())
}

/// Decode a WBMP file with caller-supplied resource limits. Identical
/// to [`parse_wbmp`] except the [`WbmpLimits`] are taken from
/// `limits` instead of [`WbmpLimits::default`].
///
/// Use [`WbmpLimits::unbounded`] for trusted local input where the
/// decoder should allocate whatever the header asks for; otherwise
/// keep the defaults or tighten them further for your environment.
pub fn parse_wbmp_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    parse_wbmp_inner(input, limits, false)
}

/// Strict variant of [`parse_wbmp`]. Identical except the header is
/// parsed with [`parse_header_strict`] — the `FixedHeader` byte is
/// required to be exactly `0x00`. Any other value raises
/// [`WbmpError::InvalidData`].
///
/// Use this entry point when the caller wants to reject malformed /
/// non-conformant Type-0 files at the wire-format level rather than
/// silently accept a byte the spec does not currently assign meaning
/// to. The lax [`parse_wbmp`] is forward-compatible with hypothetical
/// Type-0 extensions; this one is not.
pub fn parse_wbmp_strict(input: &[u8]) -> Result<WbmpImage> {
    parse_wbmp_strict_with_limits(input, &WbmpLimits::default())
}

/// Strict variant of [`parse_wbmp_with_limits`]. Same strict
/// `FixedHeader == 0x00` enforcement as [`parse_wbmp_strict`],
/// combined with caller-supplied [`WbmpLimits`].
pub fn parse_wbmp_strict_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    parse_wbmp_inner(input, limits, true)
}

fn parse_wbmp_inner(input: &[u8], limits: &WbmpLimits, strict: bool) -> Result<WbmpImage> {
    let header = if strict {
        parse_header_strict(input)?
    } else {
        parse_header(input)?
    };

    if header.width > limits.max_width {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: width {} exceeds max_width {}",
            header.width, limits.max_width
        )));
    }
    if header.height > limits.max_height {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: height {} exceeds max_height {}",
            header.height, limits.max_height
        )));
    }

    let layout = PlaneLayout::new(header.width, header.height)
        .map_err(|msg| WbmpError::invalid(msg.to_string()))?;

    if layout.total_bytes > limits.max_pixel_bytes {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: pixel-data size {} exceeds max_pixel_bytes {}",
            layout.total_bytes, limits.max_pixel_bytes
        )));
    }

    let body = &input[header.data_offset..];
    if body.len() < layout.total_bytes {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel data truncated (need {} bytes, got {})",
            layout.total_bytes,
            body.len()
        )));
    }

    // Byte layout matches our plane format directly — copy verbatim.
    // We allow trailing bytes past `layout.total_bytes` (some encoders
    // pad to even byte boundaries); we just drop them.
    let data = body[..layout.total_bytes].to_vec();

    Ok(WbmpImage {
        width: header.width,
        height: header.height,
        pixel_format: WbmpPixelFormat::MonoWhite,
        planes: vec![WbmpPlane {
            stride: layout.stride,
            data,
        }],
        pts: None,
    })
}

/// Decode a WBMP file into the requested [`WbmpPixelFormat`] using the
/// default [`WbmpLimits`].
///
/// Behaves identically to [`parse_wbmp`] when `target` is
/// [`WbmpPixelFormat::MonoWhite`]. When `target` is
/// [`WbmpPixelFormat::MonoBlack`] every bit is inverted before being
/// returned, and any padding bits in the last byte of every row are
/// masked back to zero so they stay distinguishable from real black
/// pixels — matching the symmetric convention the encoder uses when it
/// accepts a `MonoBlack` plane.
///
/// The wire format never changes — this is a decode-side convenience
/// only, equivalent to (but cheaper than) parsing first and walking
/// the plane afterwards because the inversion happens in-place during
/// the decode-time row copy.
pub fn parse_wbmp_as(input: &[u8], target: WbmpPixelFormat) -> Result<WbmpImage> {
    parse_wbmp_as_with_limits(input, target, &WbmpLimits::default())
}

/// Decode a WBMP file into the requested [`WbmpPixelFormat`] with
/// caller-supplied [`WbmpLimits`].
///
/// See [`parse_wbmp_as`] for the polarity semantics and
/// [`parse_wbmp_with_limits`] for the limits semantics.
pub fn parse_wbmp_as_with_limits(
    input: &[u8],
    target: WbmpPixelFormat,
    limits: &WbmpLimits,
) -> Result<WbmpImage> {
    let mut image = parse_wbmp_with_limits(input, limits)?;
    convert_plane_polarity(&mut image, target);
    Ok(image)
}

/// Apply an in-place polarity transform to bring the on-disk
/// `MonoWhite` plane returned by [`parse_wbmp_with_limits`] into the
/// caller's requested [`WbmpPixelFormat`].
///
/// No-op when `target == MonoWhite`. For `MonoBlack` we invert every
/// payload byte and then zero out the padding bits in the last byte of
/// every row (so a `1`-bit in the plane unambiguously means "black",
/// rather than "either black or unused padding bit").
fn convert_plane_polarity(image: &mut WbmpImage, target: WbmpPixelFormat) {
    if image.pixel_format == target {
        return;
    }
    match target {
        WbmpPixelFormat::MonoWhite => {
            // The decode path always emits MonoWhite verbatim — this
            // branch only fires if the caller passes a MonoBlack image
            // back through us. Same masking + inversion logic round-
            // trips cleanly to MonoWhite.
            invert_plane_in_place(image);
            image.pixel_format = WbmpPixelFormat::MonoWhite;
        }
        WbmpPixelFormat::MonoBlack => {
            invert_plane_in_place(image);
            image.pixel_format = WbmpPixelFormat::MonoBlack;
        }
    }
}

/// Flip every bit of the single packed plane in `image` and re-zero
/// the padding bits in the last byte of every row.
///
/// Padding handling matters: after inverting, what used to be a
/// row's trailing-zero padding becomes a trailing-one run that
/// callers iterating bit-by-bit would mistake for real foreground
/// pixels. We mask it back to zero so the plane stays well-formed
/// regardless of polarity.
///
/// The per-row mask comes from the [`PlaneLayout`] typed primitive
/// (`last_byte_pad_mask`), which is the same byte the encoder's
/// `MonoBlack` ingress branch uses — keeping the two sites in sync on
/// the masking convention.
fn invert_plane_in_place(image: &mut WbmpImage) {
    let plane = &mut image.planes[0];
    for b in plane.data.iter_mut() {
        *b = !*b;
    }
    // `PlaneLayout::new` only fails on usize overflow; we already
    // allocated the plane bytes for this (width, height) so by
    // construction the layout is computable.
    let layout = match PlaneLayout::new(image.width, image.height) {
        Ok(l) => l,
        Err(_) => return,
    };
    // `last_byte_pad_mask` is 0xFF when there are no padding bits to
    // mask — in that case the per-row AND below is a no-op, but
    // skipping the loop also skips the per-row branch + index
    // computation. The width=0 case still has stride=0 and would
    // produce a malformed `last = -1` index without this guard.
    if layout.last_byte_pad_mask != 0xFF && layout.stride > 0 {
        for y in 0..layout.height as usize {
            let last = y * layout.stride + (layout.stride - 1);
            if last < plane.data.len() {
                plane.data[last] &= layout.last_byte_pad_mask;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::write_header;

    #[test]
    fn parse_minimal_1x1_white() {
        // 1×1 image, single pixel = white (bit 7 of byte 0 set).
        let mut buf = Vec::new();
        write_header(1, 1, &mut buf);
        buf.push(0b1000_0000);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixel_format, WbmpPixelFormat::MonoWhite);
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

    // --- Polarity (MonoWhite ↔ MonoBlack) decode tests. ---

    #[test]
    fn parse_as_monowhite_matches_parse_wbmp() {
        // parse_wbmp_as(MonoWhite) must be byte-for-byte identical to
        // parse_wbmp for the same input.
        let mut buf = Vec::new();
        write_header(11, 3, &mut buf);
        let body = [0xAA, 0x80, 0x55, 0xA0, 0xC3, 0x40];
        buf.extend_from_slice(&body);
        let a = parse_wbmp(&buf).unwrap();
        let b = parse_wbmp_as(&buf, WbmpPixelFormat::MonoWhite).unwrap();
        assert_eq!(a.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(b.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(a.planes[0].data, b.planes[0].data);
    }

    #[test]
    fn parse_as_monoblack_inverts_full_byte_rows() {
        // 8×1 byte-aligned row: the polarity flip is a clean `!byte`
        // with no padding to mask.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0b1010_0110);
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap();
        assert_eq!(img.pixel_format, WbmpPixelFormat::MonoBlack);
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
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap();
        // Inverted: 0x53 in byte 0; byte 1 inversion would give
        // 0x1F, but masking the 5 padding bits zeroes them → 0x00.
        assert_eq!(img.planes[0].data, [0x53, 0x00]);
    }

    #[test]
    fn parse_as_monoblack_roundtrips_through_inversion() {
        // Decoding to MonoBlack twice (via parse_as → encode → parse_as)
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
        // trip target is well-formed input → MonoWhite verbatim.
        let pad_bits = stride * 8 - 159;
        let mask: u8 = 0xFFu8 << pad_bits;
        for y in 0..5 {
            let last = y * stride + (stride - 1);
            bits[last] &= mask;
        }
        let encoded = crate::encoder::encode_wbmp(159, 5, &bits).unwrap();
        // First decode: MonoBlack (inverted with padding masked).
        let blk = parse_wbmp_as(&encoded, WbmpPixelFormat::MonoBlack).unwrap();
        assert_eq!(blk.pixel_format, WbmpPixelFormat::MonoBlack);
        // Manually invert + mask: must match the MonoBlack plane bit-
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
        // Limit checks fire before the polarity flip — a MonoBlack
        // decode of an over-sized header must still raise
        // LimitExceeded, not run through the inversion loop on
        // unallocated memory.
        let mut buf = Vec::new();
        write_header(16_000, 16_000, &mut buf);
        let err = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap_err();
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
        let w = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoWhite, &lim).unwrap();
        let b = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoBlack, &lim).unwrap();
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
        assert_eq!(lax.pixel_format, strict.pixel_format);
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
