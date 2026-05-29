//! WBMP Type-0 encoder.
//!
//! Two standalone entry points:
//!
//! * [`encode_wbmp`] — accept an already-packed mono plane (1 bit per
//!   pixel, MSB-first, 1 = white, rows padded to a byte boundary) and
//!   wrap it with a Type-0 header. Cheap: just a header prefix + the
//!   pixel bytes themselves.
//! * [`encode_wbmp_from_threshold`] — convenience wrapper that takes
//!   a tightly-packed 8-bit grayscale buffer (one byte per pixel, no
//!   row padding) and a brightness threshold, then produces the
//!   1-bit-per-pixel plane and a complete WBMP file in one call.
//!
//! Both functions emit the same bytes for the same logical input, so
//! `parse_wbmp(encode_wbmp(w, h, bits)).unwrap()` round-trips bit
//! exactly.

use crate::error::{Result, WbmpError};
use crate::header::write_header;
use crate::image::WbmpImage;

#[cfg(feature = "registry")]
use oxideav_core::Encoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, TimeBase};

/// Encode a WBMP Type-0 file from an already-packed monochrome bit
/// plane.
///
/// `mono_bits` must be exactly `ceil(width / 8) * height` bytes long,
/// with bits packed MSB-first within each byte; bit `1` is white,
/// bit `0` is black, and trailing bits in the last byte of every row
/// are ignored (they should be zero by convention).
pub fn encode_wbmp(width: u32, height: u32, mono_bits: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp: zero dimension (width={width}, height={height})"
        )));
    }
    let stride = WbmpImage::row_stride(width);
    let expected = stride
        .checked_mul(height as usize)
        .ok_or_else(|| WbmpError::invalid("encode_wbmp: width × height overflows usize"))?;
    if mono_bits.len() != expected {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp: mono_bits length {} != stride*height {expected}",
            mono_bits.len()
        )));
    }

    // Header is at most 1 + 1 + 5 + 5 = 12 bytes (worst case) — pre-
    // allocate accordingly so the body push doesn't reallocate.
    let mut out = Vec::with_capacity(12 + expected);
    write_header(width, height, &mut out);
    out.extend_from_slice(mono_bits);
    Ok(out)
}

/// Convenience helper: threshold an 8-bit grayscale buffer (one byte
/// per pixel, row-major, no padding) into a 1-bit plane and wrap it
/// in a WBMP Type-0 file.
///
/// Pixels with grayscale value `>= threshold` become white (bit 1),
/// pixels below become black (bit 0). Choose `threshold = 128` for
/// the standard mid-grey cutoff, `threshold = 1` to drop only true
/// black, etc.
///
/// `gray.len()` must equal `width * height`.
pub fn encode_wbmp_from_threshold(
    width: u32,
    height: u32,
    gray: &[u8],
    threshold: u8,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp_from_threshold: zero dimension (width={width}, height={height})"
        )));
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| {
            WbmpError::invalid("encode_wbmp_from_threshold: width × height overflows usize")
        })?;
    if gray.len() != pixel_count {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp_from_threshold: gray length {} != width*height {pixel_count}",
            gray.len()
        )));
    }

    let stride = WbmpImage::row_stride(width);
    let mut bits = vec![0u8; stride * height as usize];
    let w = width as usize;
    let full_bytes = w / 8;
    let tail_bits = w % 8;

    for y in 0..height as usize {
        let row_in = &gray[y * w..(y + 1) * w];
        let row_out = &mut bits[y * stride..(y + 1) * stride];

        // Pack eight samples per output byte without a branch on the
        // hot loop body. `>= threshold` becomes a single comparison
        // per sample, and the eight bit positions OR together into
        // one byte with no in-place read-modify-write. The compiler
        // unrolls this cleanly on every backend we ship.
        for (out_byte, in_chunk) in row_out
            .iter_mut()
            .zip(row_in.chunks_exact(8))
            .take(full_bytes)
        {
            *out_byte = ((in_chunk[0] >= threshold) as u8) << 7
                | ((in_chunk[1] >= threshold) as u8) << 6
                | ((in_chunk[2] >= threshold) as u8) << 5
                | ((in_chunk[3] >= threshold) as u8) << 4
                | ((in_chunk[4] >= threshold) as u8) << 3
                | ((in_chunk[5] >= threshold) as u8) << 2
                | ((in_chunk[6] >= threshold) as u8) << 1
                | ((in_chunk[7] >= threshold) as u8);
        }

        // Final partial byte (`width % 8 != 0`): pack the remaining
        // 1..=7 samples MSB-first into the last byte of the row,
        // leaving the unused low bits at zero (the WBMP convention).
        if tail_bits != 0 {
            let base = full_bytes * 8;
            let mut b: u8 = 0;
            for k in 0..tail_bits {
                if row_in[base + k] >= threshold {
                    b |= 1 << (7 - k);
                }
            }
            row_out[full_bytes] = b;
        }
    }

    encode_wbmp(width, height, &bits)
}

// --------------------------------------------------------------------
// Registry-side Encoder trait surface.
// --------------------------------------------------------------------

#[cfg(feature = "registry")]
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    Ok(Box::new(WbmpEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct WbmpEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Encoder for WbmpEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "WBMP encoder: expected video frame",
                ))
            }
        };
        let format = self.out_params.pixel_format.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: pixel_format missing in CodecParameters")
        })?;
        let width = self.out_params.width.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: width missing in CodecParameters")
        })?;
        let height = self.out_params.height.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: height missing in CodecParameters")
        })?;

        if vf.planes.is_empty() {
            return Err(oxideav_core::Error::invalid("WBMP encoder: no planes"));
        }
        let plane = &vf.planes[0];

        let bytes = match format {
            // Wire layout is MSB-first / 1=white. MonoWhite already
            // uses that bit polarity, MonoBlack inverts it.
            PixelFormat::MonoWhite => encode_wbmp(width, height, &plane.data)?,
            PixelFormat::MonoBlack => {
                let mut inverted = plane.data.clone();
                for b in inverted.iter_mut() {
                    *b = !*b;
                }
                // Mask off any padding bits in the last byte of each
                // row: width may not be a multiple of 8, and inverting
                // would flip the padding zeros to ones, which the
                // decoder ignores but is messy on disk.
                let stride = WbmpImage::row_stride(width);
                let pad_bits = (stride * 8) - width as usize;
                if pad_bits > 0 {
                    let mask: u8 = !((1u16 << pad_bits) - 1) as u8;
                    for y in 0..height as usize {
                        let last = y * stride + (stride - 1);
                        if last < inverted.len() {
                            inverted[last] &= mask;
                        }
                    }
                }
                encode_wbmp(width, height, &inverted)?
            }
            // Convenience: accept an 8-bit Gray plane and threshold
            // at the standard mid-grey cutoff.
            PixelFormat::Gray8 => encode_wbmp_from_threshold(width, height, &plane.data, 128)?,
            other => {
                return Err(oxideav_core::Error::invalid(format!(
                    "WBMP encoder: unsupported pixel format {other:?}"
                )))
            }
        };

        self.pending = Some(bytes);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::parse_wbmp;

    #[test]
    fn roundtrip_8x8_pattern() {
        // 8×8 checkerboard: alternating 0xAA / 0x55 byte rows.
        let bits = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];
        let buf = encode_wbmp(8, 8, &bits).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, bits);
    }

    #[test]
    fn roundtrip_padded_dimension() {
        // 11×3 — stride = 2 bytes, total 6 body bytes.
        let bits = [
            0b1010_1010,
            0b1010_0000, // row 0
            0b1111_0000,
            0b1111_0000, // row 1
            0b0000_0000,
            0b0000_0000, // row 2 (all black)
        ];
        let buf = encode_wbmp(11, 3, &bits).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, bits);
    }

    #[test]
    fn encode_rejects_short_buffer() {
        // 16×1 needs 2 bytes; pass 1.
        let err = encode_wbmp(16, 1, &[0xFF]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn encode_rejects_zero_dim() {
        assert!(encode_wbmp(0, 1, &[]).is_err());
        assert!(encode_wbmp(1, 0, &[]).is_err());
    }

    #[test]
    fn threshold_helper_simple() {
        // 4×1 grayscale [255, 200, 50, 0]; threshold 128 → bits 1,1,0,0
        // → packed 0b1100_0000 = 0xC0.
        let buf = encode_wbmp_from_threshold(4, 1, &[255, 200, 50, 0], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, [0xC0]);
    }

    #[test]
    fn threshold_helper_threshold_at_boundary() {
        // value == threshold counts as white per spec text "≥".
        let buf = encode_wbmp_from_threshold(2, 1, &[128, 127], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].data[0], 0b1000_0000);
    }

    #[test]
    fn threshold_helper_rejects_wrong_size() {
        let err = encode_wbmp_from_threshold(3, 1, &[0, 0], 128).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }
}
