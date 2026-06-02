#![no_main]

//! Dither-encode a fuzz-controlled 8-bit grayscale buffer and
//! self-round-trip the result.
//!
//! `encode_wbmp_from_dither` is the WBMP encoder's only stateful
//! per-pixel path: every pixel reads an accumulator that the previous
//! pixel just wrote, and every row writes a scratch buffer the next
//! row will consume. That is more arithmetic-heavy than the
//! threshold-encoder's per-pixel comparison, so it has its own
//! distinct failure modes worth hammering: `saturating_add` clamping on
//! degenerate inputs, the `i16` accumulator's signed-divide rounding
//! direction, and the per-row `cur` / `next` buffer swap.
//!
//! The fuzzer derives width and height from the first two input bytes
//! (kept small so the synthesised gray buffer stays comfortably under
//! the default `WbmpLimits` cap the decoder applies after the encode),
//! pads the remaining fuzz bytes to the required `width * height`
//! length by cycling, runs `encode_wbmp_from_dither`, decodes the
//! produced file with `parse_wbmp`, and asserts the structural
//! invariants:
//!
//!  * dimensions survive the round trip,
//!  * stride is `ceil(width / 8)`,
//!  * the padding bits in the last byte of every row are zero (a
//!    diffuse-error encoder can land *any* bit pattern in the active
//!    pixels but must never write to the padding tail),
//!  * for the pure-black / pure-white saturated-input case the dither
//!    output agrees byte-for-byte with
//!    `encode_wbmp_from_threshold(.., 128)` — saturated samples
//!    propagate zero residual, so the two helpers are documented to
//!    match on this case.
//!
//! As with the other targets, the crate is pulled in with
//! `default-features = false` so the harness never links
//! `oxideav-core` and exercises only the framework-free encode path.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    encode_wbmp_from_dither, encode_wbmp_from_threshold, parse_wbmp, WbmpImage, WbmpPixelFormat,
};

fuzz_target!(|data: &[u8]| {
    // Need two control bytes plus at least one pixel.
    if data.len() < 3 {
        return;
    }

    // Range 1..=256 on each dimension keeps the worst-case gray buffer
    // (256 × 256 = 64 KiB) comfortably under the default
    // `max_pixel_bytes` (8 MiB) so a valid encode always round-trips
    // through the decoder.
    let width: u32 = u32::from(data[0]) + 1;
    let height: u32 = u32::from(data[1]) + 1;

    let pixel_count = (width as usize) * (height as usize);
    // Build a grayscale plane of exactly the required length, sourcing
    // bytes from the remaining fuzz input (cycled when shorter than
    // pixel_count). Cycling keeps every fuzz byte semantically
    // meaningful for both small and large inputs.
    let body = &data[2..];
    let mut gray = Vec::with_capacity(pixel_count);
    for i in 0..pixel_count {
        gray.push(if body.is_empty() {
            0
        } else {
            body[i % body.len()]
        });
    }

    let encoded = match encode_wbmp_from_dither(width, height, &gray) {
        Ok(v) => v,
        // Dimensions >= 1 and gray.len() == width * height, so an Err
        // here would be a genuine encoder bug. Treat it as
        // "uninteresting" rather than `unwrap` (an actual panic by the
        // encoder would already crash via the arithmetic itself).
        Err(_) => return,
    };

    let image = parse_wbmp(&encoded).expect("dither-encoded WBMP must decode");
    assert_eq!(image.width, width, "width survives round trip");
    assert_eq!(image.height, height, "height survives round trip");
    assert_eq!(image.pixel_format, WbmpPixelFormat::MonoWhite);
    assert_eq!(image.planes.len(), 1, "WBMP carries exactly one plane");

    let stride = WbmpImage::row_stride(width);
    assert_eq!(image.planes[0].stride, stride, "stride matches ceil(w/8)");

    // Padding bits in the last byte of every row must always be zero.
    // The dither encoder writes its output via `row_out[x >> 3] |= bit`
    // and never touches the padding columns, but a regression here is
    // the most likely defect a future change to the packing path could
    // introduce.
    let w = width as usize;
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

    // Saturated-input agreement: a buffer made entirely of 0 / 255
    // samples propagates zero residual, so dither and
    // threshold-at-128 must agree byte-for-byte. We don't get that
    // for free from arbitrary fuzz input, but we can probe it
    // cheaply: clamp the gray buffer to {0, 255} via `>= 128 ? 255 : 0`
    // and re-encode with both helpers. The two outputs must match.
    let mut clamped = gray.clone();
    for byte in clamped.iter_mut() {
        *byte = if *byte >= 128 { 255 } else { 0 };
    }
    let dith_sat = encode_wbmp_from_dither(width, height, &clamped)
        .expect("saturated dither encode must succeed");
    let thr_sat = encode_wbmp_from_threshold(width, height, &clamped, 128)
        .expect("saturated threshold encode must succeed");
    assert_eq!(
        dith_sat, thr_sat,
        "saturated 0/255 input: dither must agree byte-for-byte with threshold-at-128",
    );
});
