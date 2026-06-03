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
//! * [`encode_wbmp_from_dither`] — alternative wrapper that runs a
//!   Floyd–Steinberg error-diffusion quantiser over the same 8-bit
//!   grayscale input before packing. Useful for photographic source
//!   material where a hard threshold collapses every mid-tone to a
//!   flat region. Reference: R. W. Floyd and L. Steinberg, "An
//!   adaptive algorithm for spatial greyscale", Proc. SID
//!   17/2 (1976), pp. 75–77.
//!
//! All three functions emit the same wire layout for the same
//! resulting bit plane, so
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

/// Floyd–Steinberg error-diffusion quantiser → WBMP Type-0 file.
///
/// Walks the 8-bit grayscale input left-to-right, top-to-bottom. At
/// each pixel the running luminance value is compared against 128:
/// values `>= 128` emit a white bit (1) and clamp the quantised
/// output to 255; values below emit a black bit (0) with output 0.
/// The signed error `actual - quantised` (range −128..=127) is then
/// diffused to the four forward neighbours in the classic
/// 7/16, 3/16, 5/16, 1/16 distribution:
///
/// ```text
///                   X    7/16
///         3/16   5/16   1/16
/// ```
///
/// The accumulator uses i16 so the propagated error never wraps; the
/// outgoing pixel is clamped back into 0..=255 before the next
/// pixel's threshold. This produces a stippled rendering that
/// preserves local average luminance — markedly better than a hard
/// threshold for photographic material at the cost of one extra
/// scratch row of i16 storage.
///
/// `gray.len()` must equal `width * height`. Inputs are consumed by
/// value-copy into the scratch buffer; the caller's buffer is not
/// mutated.
///
/// Reference: R. W. Floyd and L. Steinberg, "An adaptive algorithm
/// for spatial greyscale", Proc. SID 17/2 (1976), pp. 75–77.
pub fn encode_wbmp_from_dither(width: u32, height: u32, gray: &[u8]) -> Result<Vec<u8>> {
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

    // Two i16 row buffers: `cur` is the row we're quantising now,
    // `next` accumulates the forward-diffused errors for the row
    // below. Swap on each row boundary so the algorithm runs in
    // O(width) extra space rather than holding the whole frame in
    // i16.
    let mut cur: Vec<i16> = Vec::with_capacity(w);
    let mut next: Vec<i16> = vec![0; w];

    // Seed the first row from the input.
    cur.extend(gray[..w].iter().map(|&g| g as i16));

    for y in 0..h {
        let row_out = &mut bits[y * stride..(y + 1) * stride];

        // Pack output bits into a u8 accumulator, flushing once per
        // 8 pixels rather than doing a read-modify-write store on
        // every pixel. The bit positions never collide (each pixel
        // sets exactly bit `7 - (x & 7)` of byte `x >> 3`), so this
        // produces a byte-identical plane to the per-pixel `|=` form.
        // r225 depth-mode: 1-store-per-8-pixels in the dither path,
        // matching the chunked-eight pack the threshold path already
        // uses.
        let mut acc: u8 = 0;
        for x in 0..w {
            // Quantise to the nearest of {0, 255}; the boundary 128
            // matches `encode_wbmp_from_threshold`'s "≥ 128 = white"
            // convention so the two helpers agree on flat-grey input.
            let (out_byte, out_value) = if cur[x] >= 128 {
                (1u8, 255i16)
            } else {
                (0u8, 0i16)
            };
            // Accumulate the output bit MSB-first; flush at byte
            // boundaries.
            acc |= out_byte << (7 - (x & 7));
            if (x & 7) == 7 {
                row_out[x >> 3] = acc;
                acc = 0;
            }

            // Diffuse the residual to the four forward neighbours.
            // The weights sum to 16, and we apply each multiplied-up
            // numerator with a divide-by-16 — the `+ 8` rounds the
            // signed division to nearest rather than toward zero, so
            // the diffused error stays symmetric around 0 for any
            // residual sign.
            let err = cur[x] - out_value;
            if err != 0 {
                if x + 1 < w {
                    cur[x + 1] = cur[x + 1].saturating_add(div_round_i16(err * 7, 16));
                }
                if y + 1 < h {
                    if x > 0 {
                        next[x - 1] = next[x - 1].saturating_add(div_round_i16(err * 3, 16));
                    }
                    next[x] = next[x].saturating_add(div_round_i16(err * 5, 16));
                    if x + 1 < w {
                        next[x + 1] = next[x + 1].saturating_add(div_round_i16(err, 16));
                    }
                }
            }
        }
        // Flush any partial trailing byte (`width % 8 != 0`). Unused
        // low bits of `acc` stay zero by construction, matching the
        // WBMP padding convention.
        if (w & 7) != 0 {
            row_out[w >> 3] = acc;
        }

        // Advance to the next row: `next` becomes the new `cur` and
        // is biased with the next input row's grayscale values; the
        // old `cur` is reset to zeros for the row after that.
        if y + 1 < h {
            cur.clear();
            let next_in = &gray[(y + 1) * w..(y + 2) * w];
            cur.extend(next.iter().zip(next_in.iter()).map(|(&e, &g)| g as i16 + e));
            for slot in next.iter_mut() {
                *slot = 0;
            }
        }
    }

    encode_wbmp(width, height, &bits)
}

/// Round-half-to-nearest signed integer division by a small positive
/// divisor. `div` must be > 0. Used by [`encode_wbmp_from_dither`] to
/// distribute Floyd–Steinberg residuals symmetrically around zero.
#[inline]
fn div_round_i16(num: i16, div: i16) -> i16 {
    debug_assert!(div > 0);
    if num >= 0 {
        (num + div / 2) / div
    } else {
        -(((-num) + div / 2) / div)
    }
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

    #[test]
    fn dither_helper_pure_black_and_white_pass_through() {
        // Saturated inputs have zero residual to diffuse, so dither
        // and threshold-at-128 must agree byte-for-byte.
        let gray = [255u8, 255, 255, 255, 0, 0, 0, 0];
        let dith = encode_wbmp_from_dither(8, 1, &gray).unwrap();
        let thr = encode_wbmp_from_threshold(8, 1, &gray, 128).unwrap();
        assert_eq!(dith, thr);

        let img = parse_wbmp(&dith).unwrap();
        assert_eq!(img.planes[0].data, [0b1111_0000]);
    }

    #[test]
    fn dither_helper_zero_dim_rejected() {
        assert!(encode_wbmp_from_dither(0, 1, &[]).is_err());
        assert!(encode_wbmp_from_dither(1, 0, &[]).is_err());
    }

    #[test]
    fn dither_helper_rejects_wrong_size() {
        let err = encode_wbmp_from_dither(3, 1, &[0, 0]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn dither_helper_preserves_average_on_flat_midtone() {
        // A 32×32 flat patch of value 128 should quantise to roughly
        // half white / half black under Floyd–Steinberg. With a hard
        // threshold-at-128 it'd be 100% white; dither must do
        // measurably better.
        let w = 32u32;
        let h = 32u32;
        let gray = vec![128u8; (w * h) as usize];
        let buf = encode_wbmp_from_dither(w, h, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let ones: u32 = img.planes[0].data.iter().map(|b| b.count_ones()).sum();
        // 32×32 = 1024 bits. Average 128/255 ≈ 0.50196 ≈ 514 white
        // bits would be perfect; allow a generous ±5% band to absorb
        // boundary clamping at the row ends.
        let total = w * h;
        let lo = total * 45 / 100;
        let hi = total * 55 / 100;
        assert!(
            (lo..=hi).contains(&ones),
            "dither produced {ones} white bits of {total}; expected {lo}..={hi}"
        );
    }

    #[test]
    fn dither_helper_roundtrips_width_with_padding() {
        // 11×3 — exercises the stride=2 padding-tail path the
        // threshold helper also handles.
        let gray = [
            255u8, 200, 50, 0, 255, 200, 50, 0, 128, 64, 192, //
            64, 200, 50, 255, 0, 50, 200, 64, 192, 128, 0, //
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let buf = encode_wbmp_from_dither(11, 3, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.width, 11);
        assert_eq!(img.height, 3);
        assert_eq!(img.planes[0].stride, 2);
        // Padding bits in the last byte of every row must be zero
        // (low 5 bits of each row's second byte).
        for y in 0..3 {
            let last = y * 2 + 1;
            assert_eq!(
                img.planes[0].data[last] & 0b0001_1111,
                0,
                "row {y} padding bits non-zero"
            );
        }
    }

    #[test]
    fn dither_helper_full_byte_plus_tail_bits() {
        // 11×1 grayscale: a saturated checkerboard 255/0/255/.../255.
        // Saturated inputs propagate zero residual under
        // Floyd-Steinberg, so the dither output must equal what a
        // bit-by-bit reference (set bit `7 - (x % 8)` of byte `x / 8`
        // when gray[x] >= 128) produces. This locks the r225
        // accumulator-flush pack against any future change that
        // accidentally drops a bit on the byte boundary or leaves
        // padding bits non-zero.
        let gray = [255u8, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255];
        let buf = encode_wbmp_from_dither(11, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        // Expected pack: bits 1,0,1,0,1,0,1,0 (byte 0) + 1,0,1 in
        // bits 7,6,5 of byte 1 (padding bits 4..0 are zero).
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, [0b1010_1010, 0b1010_0000]);
    }

    #[test]
    fn dither_helper_byte_boundary_padding_stays_zero() {
        // 9×1 grayscale: one full byte (bits 7..0) + one tail bit in
        // bit 7 of byte 1. The remaining low 7 bits of byte 1 are
        // padding and must be zero.
        let gray = [255u8; 9];
        let buf = encode_wbmp_from_dither(9, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, [0b1111_1111, 0b1000_0000]);
    }

    #[test]
    fn dither_helper_horizontal_ramp_is_balanced() {
        // 64-pixel left-to-right ramp from 0 to 255. Half above
        // 128, half below before dithering; the diffused output
        // should be close to half-and-half overall.
        let w = 64u32;
        let mut gray = Vec::with_capacity(w as usize);
        for x in 0..w {
            gray.push(((x * 255) / (w - 1)) as u8);
        }
        let buf = encode_wbmp_from_dither(w, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let ones: u32 = img.planes[0].data.iter().map(|b| b.count_ones()).sum();
        // Average grayscale of the ramp is ≈ 127.5 / 255 ≈ 50%; the
        // 1-row pass has nowhere to diffuse vertically so a ±10%
        // band absorbs the residual-at-EOL clamping.
        assert!(
            (24..=40).contains(&ones),
            "ramp dithered to {ones} of 64 white bits"
        );
    }
}
