//! Hard-asserted self-roundtrip tests for `oxideav-wbmp`.
//!
//! Each case builds a synthetic image (programmatically, no fixtures
//! on disk), encodes it via the public [`encode_wbmp`] /
//! [`encode_wbmp_from_threshold`] API, parses it back via
//! [`parse_wbmp`], and checks every byte of the recovered plane
//! matches the input bit-for-bit. Header fields (`width`, `height`,
//! `pixel_format`) are also asserted exactly.
//!
//! These run on the default-feature build (registry on); the
//! standalone-build CI job covers `--no-default-features --lib` so
//! integration tests under `tests/` skipping isn't a concern.

use oxideav_wbmp::{
    encode_wbmp, encode_wbmp_from_threshold, parse_wbmp, WbmpImage, WbmpPixelFormat,
};

fn assert_roundtrip(width: u32, height: u32, bits: &[u8]) {
    let stride = WbmpImage::row_stride(width);
    assert_eq!(
        bits.len(),
        stride * height as usize,
        "test bug: bits length mismatch ({} vs {})",
        bits.len(),
        stride * height as usize,
    );
    let encoded = encode_wbmp(width, height, bits).unwrap();
    let decoded = parse_wbmp(&encoded).unwrap();
    assert_eq!(decoded.width, width);
    assert_eq!(decoded.height, height);
    assert_eq!(decoded.pixel_format, WbmpPixelFormat::MonoWhite);
    assert_eq!(decoded.planes.len(), 1);
    assert_eq!(decoded.planes[0].stride, stride);
    assert_eq!(decoded.planes[0].data, bits);
}

#[test]
fn roundtrip_8x8_solid_white() {
    let bits = vec![0xFFu8; 8];
    assert_roundtrip(8, 8, &bits);
}

#[test]
fn roundtrip_8x8_solid_black() {
    let bits = vec![0u8; 8];
    assert_roundtrip(8, 8, &bits);
}

#[test]
fn roundtrip_64x64_diagonal() {
    // 64-pixel-wide diagonal: row y has the bit at column y set.
    let stride = 8usize; // 64/8
    let mut bits = vec![0u8; stride * 64];
    for y in 0..64usize {
        let x = y;
        bits[y * stride + x / 8] |= 1 << (7 - (x % 8));
    }
    assert_roundtrip(64, 64, &bits);
}

#[test]
fn roundtrip_padded_width_159x33() {
    // Width 159 → stride 20 bytes (with 1 padding bit per row).
    // Pattern: every other row all-white, alternating rows half-white
    // half-black. Tests that the padding bit handling matches between
    // encode and decode.
    let stride = WbmpImage::row_stride(159);
    let mut bits = vec![0u8; stride * 33];
    for y in 0..33usize {
        for x in 0..159usize {
            let on = if y % 2 == 0 { true } else { x < 80 };
            if on {
                bits[y * stride + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    assert_roundtrip(159, 33, &bits);
}

#[test]
fn roundtrip_small_dimensions_force_short_mbi() {
    // Both dimensions ≤ 0x7F → both MBIs encode in 1 byte; the whole
    // header is the minimum-possible 4 bytes. Smoke-tests the
    // shortest-MBI codepath end-to-end.
    let bits = vec![0b1100_1010u8; 4]; // 4 bytes for 32×1 row? no: 32×1 → 4 bytes
    assert_roundtrip(32, 1, &bits);
}

#[test]
fn roundtrip_dimensions_force_two_byte_mbi() {
    // Width 200 (= 0xC8 > 0x7F) → 2-byte width MBI. Height 100 (=
    // 0x64 ≤ 0x7F) → 1-byte height MBI. Exercises the
    // mixed-MBI-length branch.
    let stride = WbmpImage::row_stride(200);
    let bits = vec![0b1010_1010u8; stride * 100];
    assert_roundtrip(200, 100, &bits);
}

#[test]
fn roundtrip_dimensions_force_three_byte_mbi() {
    // 16385 × 1 → width MBI is 3 bytes (16385 = 0x4001 > 0x3FFF).
    // Body is 2049 bytes — keep the row simple so the test stays fast.
    let stride = WbmpImage::row_stride(16385);
    let bits = vec![0u8; stride];
    let encoded = encode_wbmp(16385, 1, &bits).unwrap();
    let decoded = parse_wbmp(&encoded).unwrap();
    assert_eq!(decoded.width, 16385);
    assert_eq!(decoded.height, 1);
    assert_eq!(decoded.planes[0].data.len(), stride);
}

#[test]
fn threshold_helper_full_grayscale_ramp_roundtrip() {
    // 256-pixel-wide ramp 0..=255: at threshold 128 the resulting
    // bits should be 128×0 then 128×1 = 16 bytes 0x00 then 16 bytes
    // 0xFF in MSB-first packing.
    let gray: Vec<u8> = (0u32..256).map(|v| v as u8).collect();
    let encoded = encode_wbmp_from_threshold(256, 1, &gray, 128).unwrap();
    let decoded = parse_wbmp(&encoded).unwrap();
    assert_eq!(decoded.width, 256);
    assert_eq!(decoded.height, 1);
    assert_eq!(decoded.planes[0].stride, 32);
    let mut expected = vec![0u8; 32];
    for (i, byte) in expected.iter_mut().enumerate().skip(16) {
        // bits 128..=255 are white
        let _ = i;
        *byte = 0xFF;
    }
    assert_eq!(decoded.planes[0].data, expected);
}

#[test]
fn threshold_helper_2d_pattern_roundtrip() {
    // 24×16 image. Top half all bright, bottom half all dark; left
    // quarter inverted within each half. Verifies row indexing
    // stays correct on both sides.
    let w = 24usize;
    let h = 16usize;
    let mut gray = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let top = y < h / 2;
            let left = x < w / 4;
            let bright = top ^ left;
            gray[y * w + x] = if bright { 255 } else { 0 };
        }
    }
    let encoded = encode_wbmp_from_threshold(w as u32, h as u32, &gray, 128).unwrap();
    let decoded = parse_wbmp(&encoded).unwrap();
    assert_eq!(decoded.planes[0].stride, 3); // 24 bits / 8

    // Re-derive the expected packed bits from the same logic and
    // compare.
    let mut expected = vec![0u8; 3 * h];
    for y in 0..h {
        for x in 0..w {
            let top = y < h / 2;
            let left = x < w / 4;
            let bright = top ^ left;
            if bright {
                expected[y * 3 + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    assert_eq!(decoded.planes[0].data, expected);
}

#[test]
fn encoded_byte_count_matches_handcalc() {
    // 8×8: 1+1+1+1 header bytes + 8 body bytes = 12 bytes total.
    let buf = encode_wbmp(8, 8, &[0u8; 8]).unwrap();
    assert_eq!(buf.len(), 12);
    // 1×1: 1+1+1+1 + 1 = 5 bytes.
    let buf = encode_wbmp(1, 1, &[0u8; 1]).unwrap();
    assert_eq!(buf.len(), 5);
    // 200×1: 1+1+2+1 + 25 = 30 bytes.
    let buf = encode_wbmp(200, 1, &[0u8; 25]).unwrap();
    assert_eq!(buf.len(), 30);
}
