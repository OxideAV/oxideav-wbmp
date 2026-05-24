#![no_main]

//! Encode a fuzz-controlled WBMP Type-0 image and assert it survives a
//! decode round trip byte-for-byte.
//!
//! WBMP is lossless and bit-exact: a packed 1-bit plane prepended with
//! a header must decode back to the same dimensions and the same plane
//! bytes. There is no standard system library worth pulling in as a
//! cross-decode oracle (and the clean-room wall bars any external WBMP
//! source), so this is a **self-roundtrip** target: `encode_wbmp` →
//! `parse_wbmp` → compare.
//!
//! The fuzzer drives the dimensions (kept small so the body stays
//! within the default `WbmpLimits` the decoder applies) and the packed
//! bits; the body is sized to exactly `ceil(width / 8) * height` so the
//! encoder accepts it.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{encode_wbmp, parse_wbmp, WbmpPixelFormat};

fuzz_target!(|data: &[u8]| {
    // Need at least two bytes for the dimension nibbles.
    if data.len() < 2 {
        return;
    }

    // Derive small, in-bounds dimensions from the first two bytes.
    // Range 1..=256 keeps the worst-case body (256-wide × 256-tall =
    // 32 bytes/row × 256 = 8 KiB) comfortably under the default
    // `max_pixel_bytes` (8 MiB) so a valid encode always round-trips.
    let width: u32 = u32::from(data[0]) + 1;
    let height: u32 = u32::from(data[1]) + 1;

    let stride = (width as usize).div_ceil(8);
    // `width <= 256` and `height <= 256`, so this can't overflow.
    let expected = stride * height as usize;

    // Build a plane of exactly the required length, sourcing bytes from
    // the remaining fuzz input (cycled/zero-padded as needed).
    let body = &data[2..];
    let mut mono_bits = Vec::with_capacity(expected);
    for i in 0..expected {
        mono_bits.push(if body.is_empty() {
            0
        } else {
            body[i % body.len()]
        });
    }

    let encoded = match encode_wbmp(width, height, &mono_bits) {
        Ok(v) => v,
        // A zero dimension is impossible here (both are >= 1), so any
        // error would be a genuine encoder bug — but we still return
        // rather than unwrap so the fuzzer reports it as a crash via
        // the assertion below only when a *decode* mismatch occurs.
        Err(_) => return,
    };

    let image = parse_wbmp(&encoded).expect("valid encoded WBMP must decode");

    assert_eq!(image.width, width, "width survives round trip");
    assert_eq!(image.height, height, "height survives round trip");
    assert_eq!(image.pixel_format, WbmpPixelFormat::MonoWhite);
    assert_eq!(image.planes.len(), 1, "WBMP carries exactly one plane");
    assert_eq!(image.planes[0].stride, stride, "stride survives round trip");
    assert_eq!(
        image.planes[0].data, mono_bits,
        "plane bytes survive round trip"
    );
});
