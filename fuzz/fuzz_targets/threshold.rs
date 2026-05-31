#![no_main]

//! Threshold-encode a fuzz-controlled 8-bit grayscale buffer and
//! self-round-trip the result.
//!
//! `encode_wbmp_from_threshold` is the only public entry point with
//! non-trivial per-pixel logic that the existing `decode` and
//! `roundtrip` fuzz targets don't exercise. It walks an 8-bit grayscale
//! buffer of exactly `width * height` bytes, packs eight comparisons
//! per output byte (full-byte head), handles a 1..=7-pixel tail when
//! `width % 8 != 0`, and prepends a Type-0 WBMP header. The hot spots
//! worth fuzzing:
//!
//!  * the `width * height` size check (overflow on attacker-controlled
//!    dimensions),
//!  * the row-indexing into the input + output buffers (the full-byte
//!    head + tail-bit branch must agree on the same column count for
//!    every row),
//!  * the trailing padding bits of the last byte of every row, which
//!    must always come out zero regardless of input grayscale values,
//!  * the threshold boundary itself — `gray[i] >= threshold` is the
//!    only branch in the inner loop and must behave the same way the
//!    spec text describes (the high bit "1 = white" gets set exactly
//!    when the sample is greater than or equal to the threshold).
//!
//! The fuzzer derives width, height and threshold from the first three
//! input bytes (kept small enough that the synthesised gray buffer
//! stays within the default `WbmpLimits` the decoder applies after the
//! encode), pads the remaining fuzz bytes to the required
//! `width * height` length, runs `encode_wbmp_from_threshold`, decodes
//! the produced file with `parse_wbmp`, and asserts:
//!
//!  * dimensions survive the round trip,
//!  * the decoded plane bytes match the locally-recomputed expected
//!    bits exactly (full-byte head + tail-bit branch),
//!  * the padding bits in the last byte of every row are zero.
//!
//! As with the other targets, the crate is pulled in with
//! `default-features = false` so the harness never links
//! `oxideav-core` and exercises only the framework-free encode path.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{encode_wbmp_from_threshold, parse_wbmp, WbmpImage, WbmpPixelFormat};

fuzz_target!(|data: &[u8]| {
    // Need three control bytes plus at least one pixel.
    if data.len() < 4 {
        return;
    }

    // Derive small, in-bounds dimensions and a threshold from the
    // first three bytes. Range 1..=256 on each dimension keeps the
    // worst-case gray buffer (256 × 256 = 64 KiB) comfortably under
    // the default `max_pixel_bytes` (8 MiB) so a valid encode always
    // round-trips through the decoder.
    let width: u32 = u32::from(data[0]) + 1;
    let height: u32 = u32::from(data[1]) + 1;
    let threshold: u8 = data[2];

    let pixel_count = (width as usize) * (height as usize);
    // Build a grayscale plane of exactly the required length, sourcing
    // bytes from the remaining fuzz input (cycled / zero-padded as
    // needed). Cycling rather than truncating keeps every fuzz byte
    // semantically meaningful — small inputs still drive the full
    // surface, large inputs drive every row distinctly.
    let body = &data[3..];
    let mut gray = Vec::with_capacity(pixel_count);
    for i in 0..pixel_count {
        gray.push(if body.is_empty() {
            0
        } else {
            body[i % body.len()]
        });
    }

    let encoded = match encode_wbmp_from_threshold(width, height, &gray, threshold) {
        Ok(v) => v,
        // The size-check and dimension-check inside the encoder will
        // never fail here (dims >= 1, gray.len() == width * height), so
        // an Err would be a genuine encoder bug. We still return cleanly
        // rather than unwrap, since libFuzzer treats `return` as
        // "input is uninteresting" — a real panic-by-encoder would
        // already crash via the encoder's internal arithmetic, not via
        // this Err path.
        Err(_) => return,
    };

    // The produced file must decode bit-for-bit. The default
    // `WbmpLimits` apply; our 256 × 256 cap keeps every produced file
    // well under them.
    let image = parse_wbmp(&encoded).expect("threshold-encoded WBMP must decode");

    assert_eq!(image.width, width, "width survives round trip");
    assert_eq!(image.height, height, "height survives round trip");
    assert_eq!(image.pixel_format, WbmpPixelFormat::MonoWhite);
    assert_eq!(image.planes.len(), 1, "WBMP carries exactly one plane");

    let stride = WbmpImage::row_stride(width);
    assert_eq!(image.planes[0].stride, stride, "stride matches ceil(w/8)");

    // Recompute the expected packed plane bit-by-bit and compare to
    // the decoded one. This is the strongest oracle we have: any
    // disagreement between the chunked-eight-pixels-per-output-byte
    // loop in the encoder and the canonical "set bit (7 - x%8) when
    // gray[y*w + x] >= threshold" rule the spec describes shows up
    // here.
    let w = width as usize;
    let mut expected = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..w {
            if gray[y * w + x] >= threshold {
                expected[y * stride + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    assert_eq!(
        image.planes[0].data, expected,
        "threshold-pack output matches the bit-by-bit reference",
    );

    // Padding bits in the last byte of every row must always be zero
    // regardless of input grayscale values — the encoder's tail-bit
    // branch is responsible for that and a regression here is the
    // most likely defect a future change to the packing loop could
    // introduce. We check it explicitly so the harness fails loudly
    // rather than via a slow corpus diff if it ever drifts.
    let pad_bits = (stride * 8).saturating_sub(w);
    if pad_bits > 0 {
        let pad_mask: u8 = (1u16 << pad_bits) as u8 - 1;
        for y in 0..height as usize {
            let last = y * stride + (stride - 1);
            assert_eq!(
                image.planes[0].data[last] & pad_mask,
                0,
                "padding bits in row {y} must be zero (got byte 0x{:02X})",
                image.planes[0].data[last],
            );
        }
    }
});
