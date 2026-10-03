//! 8-bit grey → packed 1-bit quantisers ([`Quantize`]).
//!
//! Both kernels produce the wire layout ([`crate::PixelFormat::MonoBlack`]:
//! bit `1` = white, MSB-first, rows padded to a byte with zero bits) so
//! their output drops straight into a [`crate::WbmpImage`] /
//! [`crate::encode`] without a second pass.

use crate::image::WbmpImage;
use crate::options::Quantize;

/// Quantise `width × height` grey samples (exactly that many bytes;
/// the caller has validated the length and non-zero dimensions) to a
/// `ceil(width / 8) × height` packed plane.
pub(crate) fn quantize_gray8(width: u32, height: u32, gray: &[u8], rule: Quantize) -> Vec<u8> {
    match rule {
        Quantize::Threshold(t) => threshold_bits(width, height, gray, t),
        Quantize::Dither => dither_bits(width, height, gray),
    }
}

/// Hard threshold: samples `>= threshold` → white (bit 1).
pub(crate) fn threshold_bits(width: u32, height: u32, gray: &[u8], threshold: u8) -> Vec<u8> {
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
        // one byte with no in-place read-modify-write.
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
    bits
}

/// Floyd–Steinberg error diffusion with the mid-grey decision at 128.
///
/// Walks the input left-to-right, top-to-bottom. At each pixel the
/// running luminance value is compared against 128: values `>= 128`
/// emit a white bit (1) and clamp the quantised output to 255; values
/// below emit a black bit (0) with output 0. The signed error `actual -
/// quantised` (range −128..=127) is then diffused to the four forward
/// neighbours in the classic 7/16, 3/16, 5/16, 1/16 distribution:
///
/// ```text
///                   X    7/16
///         3/16   5/16   1/16
/// ```
///
/// The accumulator uses i16 so the propagated error never wraps; the
/// outgoing pixel is clamped back into 0..=255 before the next pixel's
/// threshold. O(width) extra space (two i16 row buffers).
pub(crate) fn dither_bits(width: u32, height: u32, gray: &[u8]) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let stride = WbmpImage::row_stride(width);
    let mut bits = vec![0u8; stride * h];

    // Two i16 row buffers: `cur` is the row we're quantising now,
    // `next` accumulates the forward-diffused errors for the row
    // below. Swap on each row boundary.
    let mut cur: Vec<i16> = Vec::with_capacity(w);
    let mut next: Vec<i16> = vec![0; w];

    // Seed the first row from the input.
    cur.extend(gray[..w].iter().map(|&g| g as i16));

    for y in 0..h {
        let row_out = &mut bits[y * stride..(y + 1) * stride];

        // Pack output bits into a u8 accumulator, flushing once per
        // 8 pixels rather than doing a read-modify-write store on
        // every pixel. The bit positions never collide (each pixel
        // sets exactly bit `7 - (x & 7)` of byte `x >> 3`).
        let mut acc: u8 = 0;
        for x in 0..w {
            // Quantise to the nearest of {0, 255}; the boundary 128
            // matches the threshold rule's "≥ 128 = white" convention
            // so the two agree on flat-grey input.
            let (out_byte, out_value) = if cur[x] >= 128 {
                (1u8, 255i16)
            } else {
                (0u8, 0i16)
            };
            acc |= out_byte << (7 - (x & 7));
            if (x & 7) == 7 {
                row_out[x >> 3] = acc;
                acc = 0;
            }

            // Diffuse the residual to the four forward neighbours.
            // The weights sum to 16; `div_round_i16` rounds the signed
            // division to nearest rather than toward zero, so the
            // diffused error stays symmetric around 0.
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
        // low bits of `acc` stay zero by construction.
        if (w & 7) != 0 {
            row_out[w >> 3] = acc;
        }

        // Advance to the next row: `next` becomes the new `cur` biased
        // with the next input row; the old `next` is reset to zeros.
        if y + 1 < h {
            cur.clear();
            let next_in = &gray[(y + 1) * w..(y + 2) * w];
            cur.extend(next.iter().zip(next_in.iter()).map(|(&e, &g)| g as i16 + e));
            for slot in next.iter_mut() {
                *slot = 0;
            }
        }
    }
    bits
}

/// Round-half-to-nearest signed integer division by a small positive
/// divisor. `div` must be > 0.
#[inline]
fn div_round_i16(num: i16, div: i16) -> i16 {
    debug_assert!(div > 0);
    if num >= 0 {
        (num + div / 2) / div
    } else {
        -(((-num) + div / 2) / div)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_packs_msb_first_with_zero_padding() {
        // 11×1: alternate 200 / 50 → 1010_1010 101 → 0xAA, 0xA0.
        let gray: Vec<u8> = (0..11).map(|i| if i % 2 == 0 { 200 } else { 50 }).collect();
        assert_eq!(threshold_bits(11, 1, &gray, 128), vec![0xAA, 0xA0]);
        // Threshold is inclusive.
        assert_eq!(threshold_bits(1, 1, &[128], 128), vec![0x80]);
        assert_eq!(threshold_bits(1, 1, &[127], 128), vec![0x00]);
    }

    #[test]
    fn dither_agrees_with_threshold_on_saturated_input() {
        let gray: Vec<u8> = (0..64).map(|i| if i % 3 == 0 { 255 } else { 0 }).collect();
        assert_eq!(dither_bits(8, 8, &gray), threshold_bits(8, 8, &gray, 128));
        assert_eq!(
            dither_bits(13, 4, &gray[..52]),
            threshold_bits(13, 4, &gray[..52], 128)
        );
    }

    #[test]
    fn dither_preserves_average_of_flat_mid_grey() {
        let gray = vec![128u8; 16 * 16];
        let bits = dither_bits(16, 16, &gray);
        let ones: u32 = bits.iter().map(|b| b.count_ones()).sum();
        // ~50 % white: 128 / 255 of 256 pixels ≈ 128.5, allow slack.
        assert!((100..=156).contains(&ones), "{ones} white pixels");
        let gray = vec![64u8; 16 * 16];
        let ones: u32 = dither_bits(16, 16, &gray)
            .iter()
            .map(|b| b.count_ones())
            .sum();
        assert!((44..=84).contains(&ones), "{ones} white pixels");
    }

    #[test]
    fn div_round_is_symmetric() {
        assert_eq!(div_round_i16(7, 16), 0);
        assert_eq!(div_round_i16(8, 16), 1);
        assert_eq!(div_round_i16(-7, 16), 0);
        assert_eq!(div_round_i16(-8, 16), -1);
        assert_eq!(div_round_i16(127 * 7, 16), 56);
        assert_eq!(div_round_i16(-128 * 7, 16), -56);
    }
}
