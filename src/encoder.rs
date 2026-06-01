//! WBMP Type-0 encoder.
//!
//! Three standalone entry points:
//!
//! * [`encode_wbmp`] — accept an already-packed mono plane (1 bit per
//!   pixel, MSB-first, 1 = white, rows padded to a byte boundary) and
//!   wrap it with a Type-0 header. Cheap: just a header prefix + the
//!   pixel bytes themselves.
//! * [`encode_wbmp_from_threshold`] — convenience wrapper that takes
//!   a tightly-packed 8-bit grayscale buffer (one byte per pixel, no
//!   row padding) and a brightness threshold, then produces the
//!   1-bit-per-pixel plane and a complete WBMP file in one call.
//! * [`encode_wbmp_from_dither`] — same input shape as
//!   `encode_wbmp_from_threshold` but uses Floyd–Steinberg error
//!   diffusion to preserve average brightness across each local
//!   region (photographic input keeps recognisable detail instead of
//!   collapsing into white-or-black bands).
//!
//! `encode_wbmp` and `encode_wbmp_from_threshold` emit the same bytes
//! for the same logical input, so
//! `parse_wbmp(encode_wbmp(w, h, bits)).unwrap()` round-trips bit
//! exactly. The dither path is non-deterministic across `threshold`
//! choices but bit-exact across repeated runs with identical inputs
//! (no internal randomness).

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
///
/// See also [`encode_wbmp_from_dither`] for the same input shape with
/// Floyd–Steinberg error diffusion instead of a hard cutoff — produces
/// considerably more pleasing 1-bit output on photographic input at
/// the cost of one extra `i16` row-pair of working buffer.
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

/// Convenience helper: convert an 8-bit grayscale buffer to a 1-bit
/// plane using Floyd–Steinberg error diffusion, then wrap it in a
/// WBMP Type-0 file.
///
/// The hard-threshold path ([`encode_wbmp_from_threshold`]) is bit-
/// exact but flattens every band-of-grey region to either solid
/// white or solid black; on photographic input that destroys most of
/// the visual detail. Floyd–Steinberg keeps the same `>= threshold`
/// per-pixel quantisation decision but pushes the quantisation
/// **error** (target value − source value) into the as-yet-unprocessed
/// neighbours, so the long-running average brightness of any local
/// region in the output matches the input. The weights are the
/// canonical 1976 distribution (Floyd & Steinberg, *An Adaptive
/// Algorithm for Spatial Greyscale*):
///
/// ```text
///       . X 7/16
///   3/16 5/16 1/16
/// ```
///
/// where `X` is the pixel currently being quantised and the four
/// fractions are the share of its quantisation error that gets added
/// to the four named neighbours (right, below-left, below, below-right).
/// Pixels at row/column edges drop the would-be-out-of-bounds share
/// silently (the lost-fraction is small and visually invisible at
/// the extreme edges).
///
/// `threshold` is the same cutoff [`encode_wbmp_from_threshold`] uses
/// (`>= threshold` becomes a white bit, `< threshold` becomes black);
/// `128` is the standard mid-grey choice.
///
/// `gray.len()` must equal `width * height`. The function allocates
/// a single working buffer of `2 * width` `i16`s (one row of
/// errors-so-far plus one row of errors-being-built for the next row),
/// independent of `height`, so the peak working set is
/// `4 * width` bytes regardless of image height.
pub fn encode_wbmp_from_dither(
    width: u32,
    height: u32,
    gray: &[u8],
    threshold: u8,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp_from_dither: zero dimension (width={width}, height={height})"
        )));
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| {
            WbmpError::invalid("encode_wbmp_from_dither: width × height overflows usize")
        })?;
    if gray.len() != pixel_count {
        return Err(WbmpError::invalid(format!(
            "encode_wbmp_from_dither: gray length {} != width*height {pixel_count}",
            gray.len()
        )));
    }

    let w = width as usize;
    let h = height as usize;
    let stride = WbmpImage::row_stride(width);
    let mut bits = vec![0u8; stride * h];

    // Two rolling error-diffusion rows.
    //   `cur` carries the error accumulated for the row we're currently
    //   quantising (read-then-add into the source sample).
    //   `nxt` accumulates the error pushed *forward* into the row below;
    //   it becomes `cur` on the next iteration.
    // Both start zeroed (the first row has no incoming error).
    let mut cur: Vec<i16> = vec![0; w];
    let mut nxt: Vec<i16> = vec![0; w];

    let thr = threshold as i16;

    for y in 0..h {
        let row_in = &gray[y * w..(y + 1) * w];
        let row_out = &mut bits[y * stride..(y + 1) * stride];

        for x in 0..w {
            // Effective sample = input + accumulated diffused error.
            // Clamp to the i16 range we work in (input is u8 so the
            // sum stays comfortably inside i16 even after many round-
            // trips of error).
            let sample = row_in[x] as i16 + cur[x];

            // Quantise.
            let (out_bit, quant_target) = if sample >= thr {
                (1u8, 255i16) // white
            } else {
                (0u8, 0i16) // black
            };

            // Pack the decision into the MSB-first byte of this column.
            if out_bit != 0 {
                row_out[x / 8] |= 1 << (7 - (x % 8));
            }

            // Distribute the quantisation error to the four canonical
            // Floyd–Steinberg neighbours. The error is the *signed*
            // discrepancy between source value and what we actually
            // emitted; positive error means we under-emitted (source
            // was brighter than what we quantised to), so neighbours
            // need a positive nudge.
            //
            // Edge handling: pixels at column 0 skip the below-left
            // share; pixels at column w-1 skip the right + below-right
            // shares. The fractions that would have gone to those
            // out-of-image neighbours are simply lost (the standard
            // "dropped edge" treatment — every Floyd–Steinberg writeup
            // discards them rather than redistributing into the
            // remaining neighbours).
            let err = sample - quant_target;
            if err != 0 {
                if x + 1 < w {
                    cur[x + 1] += (err * 7) / 16;
                }
                if x > 0 {
                    nxt[x - 1] += (err * 3) / 16;
                }
                nxt[x] += (err * 5) / 16;
                if x + 1 < w {
                    nxt[x + 1] += err / 16;
                }
            }
        }

        // Slide the error rows forward: the row we just finished
        // becomes irrelevant (its incoming error has been consumed),
        // and the row we accumulated forward becomes the next row's
        // incoming error.
        core::mem::swap(&mut cur, &mut nxt);
        for slot in nxt.iter_mut() {
            *slot = 0;
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

    // ------------------------------------------------------------
    // Floyd–Steinberg dither tests.
    // ------------------------------------------------------------

    #[test]
    fn dither_helper_solid_white_stays_white() {
        // Solid 255 input: no quantisation error anywhere, so every
        // bit must be set regardless of error-diffusion mechanics.
        // Width 11 forces a 5-bit-padded last byte we also need to
        // verify is zero.
        let buf = encode_wbmp_from_dither(11, 3, &[255u8; 33], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        for y in 0..3 {
            // Row layout: 0xFF (8 white bits) then 0xE0 (3 white bits
            // + 5 padding zeros).
            assert_eq!(img.planes[0].data[y * 2], 0xFF);
            assert_eq!(img.planes[0].data[y * 2 + 1], 0xE0);
        }
    }

    #[test]
    fn dither_helper_solid_black_stays_black() {
        // Solid 0 input: no error pushed forward, every bit clear,
        // including the padding tail of every row.
        let buf = encode_wbmp_from_dither(11, 3, &[0u8; 33], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        assert!(img.planes[0].data.iter().all(|&b| b == 0));
    }

    #[test]
    fn dither_helper_uniform_mid_grey_matches_density() {
        // Floyd–Steinberg on a solid mid-grey input must conserve
        // total brightness: a wide band of 128 should quantise to a
        // 50/50 mix of white and black bits within rounding. We don't
        // pin the exact pattern (that depends on row-by-row scanning
        // direction and weight rounding), but we *do* pin the global
        // density.
        let w: u32 = 64;
        let h: u32 = 16;
        let buf = encode_wbmp_from_dither(w, h, &vec![127u8; (w * h) as usize], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let total_bits: u32 = img.planes[0].data.iter().map(|b| b.count_ones()).sum();
        // With input = 127 (threshold = 128), each pixel falls just
        // below the threshold but error diffusion lifts roughly half
        // of them over. The exact density depends on rounding; allow
        // 35–65% as the conservative band.
        let total = w * h;
        let ratio = total_bits as f32 / total as f32;
        assert!(
            (0.35..=0.65).contains(&ratio),
            "dither density {ratio} should be near 0.5 for mid-grey input"
        );
    }

    #[test]
    fn dither_helper_ramp_is_dense_at_bright_end() {
        // Horizontal ramp 0..=255: the right edge of every row is
        // bright (> threshold), the left edge is dark. After dither,
        // the white-pixel density along any row must rise
        // monotonically (within local error-diffusion jitter) from
        // left to right. We verify the weaker invariant: the leftmost
        // quarter has fewer white pixels than the rightmost quarter.
        let w: u32 = 256;
        let h: u32 = 8;
        let mut gray = Vec::with_capacity((w * h) as usize);
        for _y in 0..h {
            for x in 0..w {
                gray.push(x as u8);
            }
        }
        let buf = encode_wbmp_from_dither(w, h, &gray, 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let stride = img.planes[0].stride;

        let mut left_white = 0u32;
        let mut right_white = 0u32;
        for y in 0..h as usize {
            let row = &img.planes[0].data[y * stride..(y + 1) * stride];
            for x in 0..(w / 4) as usize {
                if row[x / 8] & (1 << (7 - (x % 8))) != 0 {
                    left_white += 1;
                }
            }
            for x in (3 * w / 4) as usize..w as usize {
                if row[x / 8] & (1 << (7 - (x % 8))) != 0 {
                    right_white += 1;
                }
            }
        }
        assert!(
            right_white > left_white * 2,
            "dithered ramp: right quartile white={right_white} should dominate left quartile white={left_white}"
        );
    }

    #[test]
    fn dither_helper_zero_padding_in_last_byte() {
        // Width 11 produces a 5-bit-padded last byte; even on a
        // pathological input where the right-edge pixel is bright,
        // the padding bits must stay zero (they're never written by
        // the dither loop and parse_wbmp/encode_wbmp don't touch them
        // either).
        let buf = encode_wbmp_from_dither(11, 1, &[255u8; 11], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        // Padding bits = low 5 bits of the second byte.
        assert_eq!(img.planes[0].data[1] & 0x1F, 0);
    }

    #[test]
    fn dither_helper_matches_threshold_at_extremes() {
        // For inputs that are far above or far below the threshold,
        // the diffused error per pixel is too small to flip the
        // quantisation decision, so dither must agree bit-for-bit
        // with the hard-threshold path. Input = 250 (well above 128)
        // and input = 5 (well below) both fall in this regime.
        for level in [5u8, 250u8] {
            let w: u32 = 16;
            let h: u32 = 4;
            let gray = vec![level; (w * h) as usize];
            let d = encode_wbmp_from_dither(w, h, &gray, 128).unwrap();
            let t = encode_wbmp_from_threshold(w, h, &gray, 128).unwrap();
            assert_eq!(d, t, "dither must agree with threshold for level={level}");
        }
    }

    #[test]
    fn dither_helper_rejects_wrong_size() {
        let err = encode_wbmp_from_dither(3, 1, &[0, 0], 128).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn dither_helper_rejects_zero_dim() {
        assert!(encode_wbmp_from_dither(0, 1, &[], 128).is_err());
        assert!(encode_wbmp_from_dither(1, 0, &[], 128).is_err());
    }

    #[test]
    fn dither_roundtrip_decodes_to_same_dimensions() {
        // A 17×9 gradient — non-byte-aligned width forces both the
        // main-loop column-handling and the row-end edge-skip on
        // every row. Decoded image must report the right dimensions
        // and stride.
        let w: u32 = 17;
        let h: u32 = 9;
        let mut gray = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                gray.push(((x * 16) + (y * 28)) as u8);
            }
        }
        let buf = encode_wbmp_from_dither(w, h, &gray, 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.width, 17);
        assert_eq!(img.height, 9);
        assert_eq!(img.planes[0].stride, 3); // ceil(17/8) = 3
        assert_eq!(img.planes[0].data.len(), 27);
    }
}
