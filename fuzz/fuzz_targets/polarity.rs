#![no_main]

//! Encode a fuzz-controlled WBMP Type-0 image, decode it once as the
//! on-disk `MonoWhite` polarity and once as the inverted `MonoBlack`
//! polarity, and assert the two planes agree under the documented
//! in-place inversion + padding-mask transform.
//!
//! Covers `parse_wbmp_as(MonoBlack)` — the only entry point with the
//! in-place bit-inversion + per-row trailing-padding-bit re-zero logic
//! that the other four targets (`decode`, `roundtrip`, `threshold`,
//! `dither`) don't reach. Failure modes the existing targets miss:
//!
//!   * Off-by-one in the per-row "last byte of the row" indexing during
//!     the in-place padding mask (especially `width % 8 != 0` rows).
//!   * Skipping the padding mask when `pad_bits == 0` (full-byte width)
//!     where the inverted plane would otherwise be wrong.
//!   * Conditional-mask boundaries when `pad_bits` is 1 or 7.
//!
//! The fuzzer drives small dimensions (kept under the default
//! `WbmpLimits` so a valid encode always round-trips) and a packed
//! 1-bit body; the body is sized to exactly `ceil(width / 8) * height`
//! so the encoder accepts it. Trailing padding bits in each row of the
//! encoder input are pre-masked to zero so the `MonoWhite` plane is
//! well-formed and the polarity-flip's mask agreement is the only thing
//! under test.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{encode_wbmp, parse_wbmp, parse_wbmp_as, WbmpPixelFormat};

fuzz_target!(|data: &[u8]| {
    // Need at least two bytes for the dimension nibbles.
    if data.len() < 2 {
        return;
    }

    // Derive small, in-bounds dimensions from the first two bytes.
    // Range 1..=256 on each axis keeps the worst-case body (256-wide ×
    // 256-tall = 32 bytes/row × 256 = 8 KiB) under the default
    // `max_pixel_bytes` (8 MiB) so a valid encode always round-trips.
    let width: u32 = u32::from(data[0]) + 1;
    let height: u32 = u32::from(data[1]) + 1;

    let stride = (width as usize).div_ceil(8);
    let expected = stride * height as usize;

    // Build a plane of exactly the required length, sourcing bytes from
    // the remaining fuzz input (cycled / zero-padded as needed).
    let body = &data[2..];
    let mut mono_bits = vec![0u8; expected];
    for (i, byte) in mono_bits.iter_mut().enumerate() {
        if !body.is_empty() {
            *byte = body[i % body.len()];
        }
    }

    // Pre-mask the trailing padding bits in every row so the input
    // plane is well-formed (canonical) before encoding. The padding
    // bits of the *MonoWhite* on-disk layout are zero by convention;
    // re-zeroing them keeps the post-polarity-flip mask the only test
    // subject below.
    let pad_bits = stride * 8 - width as usize;
    if pad_bits > 0 && stride > 0 {
        let mask: u8 = 0xFFu8 << pad_bits;
        for y in 0..height as usize {
            let last = y * stride + (stride - 1);
            mono_bits[last] &= mask;
        }
    }

    let encoded = match encode_wbmp(width, height, &mono_bits) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Reference decode: must produce the input plane verbatim.
    let img_white = parse_wbmp(&encoded).expect("valid encoded WBMP must decode (white)");
    assert_eq!(img_white.pixel_format, WbmpPixelFormat::MonoWhite);
    assert_eq!(img_white.planes[0].stride, stride);
    assert_eq!(img_white.planes[0].data, mono_bits);

    // Polarity-flipped decode: every payload byte inverted, padding
    // bits of every row re-zeroed.
    let img_black =
        parse_wbmp_as(&encoded, WbmpPixelFormat::MonoBlack).expect("must decode (black)");
    assert_eq!(img_black.pixel_format, WbmpPixelFormat::MonoBlack);
    assert_eq!(img_black.width, width);
    assert_eq!(img_black.height, height);
    assert_eq!(img_black.planes[0].stride, stride);
    assert_eq!(img_black.planes[0].data.len(), expected);

    // Re-derive the expected MonoBlack plane from the canonical
    // MonoWhite reference: invert every byte, then re-mask padding
    // bits of the last byte of every row.
    let mut expected_black = mono_bits.clone();
    for b in expected_black.iter_mut() {
        *b = !*b;
    }
    if pad_bits > 0 && stride > 0 {
        let mask: u8 = 0xFFu8 << pad_bits;
        for y in 0..height as usize {
            let last = y * stride + (stride - 1);
            expected_black[last] &= mask;
        }
    }
    assert_eq!(
        img_black.planes[0].data, expected_black,
        "MonoBlack plane bytes must match inverted-and-padding-masked reference"
    );

    // Per-row sanity: the trailing padding bits of every row of the
    // returned MonoBlack plane must be zero — this is the only
    // post-condition the existing targets don't pin.
    if pad_bits > 0 && stride > 0 {
        let pad_mask: u8 = !(0xFFu8 << pad_bits);
        for y in 0..height as usize {
            let last = y * stride + (stride - 1);
            assert_eq!(
                img_black.planes[0].data[last] & pad_mask,
                0,
                "row {y} of MonoBlack plane has non-zero padding bits"
            );
        }
    }
});
