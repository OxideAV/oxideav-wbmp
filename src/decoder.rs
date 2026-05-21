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
use crate::header::parse_header;
use crate::image::{WbmpImage, WbmpPixelFormat, WbmpPlane};
use crate::limits::WbmpLimits;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. One packet per whole
/// WBMP file; one frame per packet.
#[cfg(feature = "registry")]
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(WbmpDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct WbmpDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Decoder for WbmpDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_wbmp(&packet.data)?;
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
    let header = parse_header(input)?;

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

    let stride = WbmpImage::row_stride(header.width);
    let expected = stride
        .checked_mul(header.height as usize)
        .ok_or_else(|| WbmpError::invalid("WBMP: width × height overflows usize"))?;

    if expected > limits.max_pixel_bytes {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: pixel-data size {expected} exceeds max_pixel_bytes {}",
            limits.max_pixel_bytes
        )));
    }

    let body = &input[header.data_offset..];
    if body.len() < expected {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel data truncated (need {expected} bytes, got {})",
            body.len()
        )));
    }

    // Byte layout matches our plane format directly — copy verbatim.
    // We allow trailing bytes past `expected` (some encoders pad to
    // even byte boundaries); we just drop them.
    let data = body[..expected].to_vec();

    Ok(WbmpImage {
        width: header.width,
        height: header.height,
        pixel_format: WbmpPixelFormat::MonoWhite,
        planes: vec![WbmpPlane { stride, data }],
        pts: None,
    })
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
