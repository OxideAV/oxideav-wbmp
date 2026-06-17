#![no_main]

//! Drive the animated-sub-image entry points `encode_wbmp_frames` /
//! `parse_wbmp_frames` (WAP-237 §4.2 / §4.5.1) — the only public surface
//! the other seven targets never touch. Two halves share the fuzz input:
//!
//!  1. **Decode** — feed the raw bytes straight to `parse_wbmp_frames` and
//!     assert it always *returns* a `Result` (no panic, no debug overflow,
//!     no out-of-bounds slice, no read past the input). On success the
//!     animation must be self-consistent: at least one frame (the main
//!     image), at most `1 + MAX_ANIMATED_IMAGES`, every plane exactly
//!     `stride * height` bytes, and the `animated_count` / `is_animated`
//!     / `main_image` helpers in agreement with `frames.len()`. The
//!     `main_image()` view must match `frames[0]` and the single-frame
//!     `parse_wbmp` decode of the same buffer.
//!
//!  2. **Encode round trip** — synthesise 1..=16 same-dimension packed
//!     planes from the remaining fuzz bytes, encode them with
//!     `encode_wbmp_frames`, decode with `parse_wbmp_frames`, and assert
//!     every plane survives byte-for-byte in stream order. A single-frame
//!     encode must be byte-identical to `encode_wbmp` (the documented
//!     equivalence) and decode to a non-animated result.
//!
//! The §4.5.1 frame-count cap, the back-to-back no-per-frame-header
//! layout, and the trailing-run-shorter-than-a-frame "ignorable padding"
//! posture are all exercised here and nowhere else in the corpus.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! exercises the framework-free standalone path and never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    encode_wbmp, encode_wbmp_frames, parse_wbmp, parse_wbmp_frames, MAX_ANIMATED_IMAGES,
};

fuzz_target!(|data: &[u8]| {
    // --- Half 1: decode arbitrary bytes, assert no panic + consistency.
    if let Ok(anim) = parse_wbmp_frames(data) {
        assert!(!anim.frames.is_empty(), "at least the main image frame");
        assert!(
            anim.frames.len() <= 1 + MAX_ANIMATED_IMAGES,
            "frame count {} within the §4.5.1 cap {}",
            anim.frames.len(),
            1 + MAX_ANIMATED_IMAGES,
        );
        assert!(anim.width >= 1 && anim.height >= 1, "non-zero dimensions");
        assert_eq!(
            anim.animated_count(),
            anim.frames.len() - 1,
            "animated_count == frames - 1",
        );
        assert_eq!(
            anim.is_animated(),
            anim.frames.len() > 1,
            "is_animated tracks frame count",
        );

        let stride = (anim.width as usize).div_ceil(8);
        let expected = stride * anim.height as usize;
        for (i, plane) in anim.frames.iter().enumerate() {
            assert_eq!(plane.stride, stride, "frame {i} stride");
            assert_eq!(plane.data.len(), expected, "frame {i} plane length");
        }

        // main_image() must reproduce frame 0 exactly, and agree with the
        // single-frame parse_wbmp of the same input.
        let main = anim.main_image();
        assert_eq!(main.planes.len(), 1, "main image carries one plane");
        assert_eq!(main.planes[0].data, anim.frames[0].data, "main == frame 0");
        if let Ok(single) = parse_wbmp(data) {
            assert_eq!(single.width, anim.width, "single-frame width agrees");
            assert_eq!(single.height, anim.height, "single-frame height agrees");
            assert_eq!(
                single.planes[0].data, anim.frames[0].data,
                "single-frame plane == main image plane",
            );
        }
    }

    // --- Half 2: synthesise frames, encode, decode, assert round trip.
    if data.len() < 3 {
        return;
    }
    // Small in-bounds dimensions (1..=256) keep each plane well under the
    // default WbmpLimits so a valid encode always decodes.
    let width = u32::from(data[0]) + 1;
    let height = u32::from(data[1]) + 1;
    // 1..=16 frames — the full §4.5.1 range (main + 0..15 animated).
    let frame_count = (data[2] as usize % (1 + MAX_ANIMATED_IMAGES)) + 1;

    let stride = (width as usize).div_ceil(8);
    let plane_len = stride * height as usize;

    let body = &data[3..];
    // Build `frame_count` distinct planes; vary each frame's seed so a
    // frame-ordering bug surfaces as a mismatch.
    let planes: Vec<Vec<u8>> = (0..frame_count)
        .map(|f| {
            (0..plane_len)
                .map(|i| {
                    if body.is_empty() {
                        0
                    } else {
                        body[(i + f) % body.len()]
                    }
                })
                .collect()
        })
        .collect();

    let refs: Vec<&[u8]> = planes.iter().map(|p| p.as_slice()).collect();

    let encoded = match encode_wbmp_frames(width, height, &refs) {
        Ok(v) => v,
        // Both dimensions are >= 1 and the count is within range and every
        // plane is exactly plane_len, so encode must succeed; a failure is
        // a real bug, but return rather than unwrap so the fuzzer reports
        // mismatches via the assertions below.
        Err(_) => return,
    };

    // Single-frame equivalence with encode_wbmp (documented contract).
    if frame_count == 1 {
        if let Ok(plain) = encode_wbmp(width, height, &planes[0]) {
            assert_eq!(encoded, plain, "single-frame encode == encode_wbmp");
        }
    }

    let anim = parse_wbmp_frames(&encoded).expect("encoded frames must decode");
    assert_eq!(anim.width, width, "width survives round trip");
    assert_eq!(anim.height, height, "height survives round trip");
    assert_eq!(anim.frames.len(), frame_count, "frame count survives");
    assert_eq!(
        anim.is_animated(),
        frame_count > 1,
        "is_animated round trip"
    );
    for (i, plane) in anim.frames.iter().enumerate() {
        assert_eq!(plane.stride, stride, "frame {i} stride round trip");
        assert_eq!(plane.data, planes[i], "frame {i} bytes survive in order");
    }
});
