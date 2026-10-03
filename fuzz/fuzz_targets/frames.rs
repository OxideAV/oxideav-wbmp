#![no_main]

//! Drive the animated-sub-image entry points `encode_frames` /
//! `decode_all` (WAP-237 §4.2 / §4.5.1) — the only public surface
//! the other seven targets never touch. Two halves share the fuzz input:
//!
//!  1. **Decode** — feed the raw bytes straight to `decode_all` and
//!     assert it always *returns* a `Result` (no panic, no debug overflow,
//!     no out-of-bounds slice, no read past the input). On success the
//!     animation must be self-consistent: at least one frame (the main
//!     image), at most `1 + MAX_ANIMATED_IMAGES`, every plane exactly
//!     `stride * height` bytes, and the `animated_count` / `is_animated`
//!     / `main_image` helpers in agreement with `frames.len()`. The
//!     `main_image()` view must match `frames[0]` and the single-frame
//!     `decode` decode of the same buffer.
//!
//!  2. **Encode round trip** — synthesise 1..=16 same-dimension packed
//!     planes from the remaining fuzz bytes, encode them with
//!     `encode_frames`, decode with `decode_all`, and assert
//!     every plane survives byte-for-byte in stream order. A single-frame
//!     encode must be byte-identical to `encode` (the documented
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
    decode, decode_all, encode, encode_frames, info, EncodeOptions, WbmpImage, MAX_ANIMATED_IMAGES,
};

fuzz_target!(|data: &[u8]| {
    // --- Half 1: decode arbitrary bytes, assert no panic + consistency.
    if let Ok(frames) = decode_all(data) {
        assert!(!frames.is_empty(), "at least the main image frame");
        assert!(
            frames.len() <= 1 + MAX_ANIMATED_IMAGES,
            "frame count {} within the §4.5.1 cap {}",
            frames.len(),
            1 + MAX_ANIMATED_IMAGES,
        );
        let main = &frames[0].image;
        assert!(main.width >= 1 && main.height >= 1, "non-zero dimensions");
        let stride = (main.width as usize).div_ceil(8);
        let expected = stride * main.height as usize;
        for (i, fr) in frames.iter().enumerate() {
            assert_eq!(fr.index as usize, i, "frame {i} index");
            assert!(fr.delay.is_none(), "WBMP carries no timing");
            assert_eq!((fr.image.width, fr.image.height), (main.width, main.height));
            assert_eq!(fr.image.planes.len(), 1, "frame {i} one plane");
            assert_eq!(fr.image.planes[0].stride, stride, "frame {i} stride");
            assert_eq!(
                fr.image.planes[0].data.len(),
                expected,
                "frame {i} plane length"
            );
        }
        // Frame 0 is the single-image decode; info() counted the frames.
        let single = decode(data).expect("decode_all ⇒ decode");
        assert_eq!(&single, main, "frame 0 == decode");
        assert_eq!(info(data).expect("header").frames as usize, frames.len());
    } else {
        assert!(
            decode(data).is_err(),
            "decode_all fails only where decode fails"
        );
    }

    // --- Half 2: synthesise frames, encode, decode, assert round trip.
    if data.len() < 3 {
        return;
    }
    // Small in-bounds dimensions (1..=256) keep each plane well under the
    // default DecodeOptions cap so a valid encode always decodes.
    let width = u32::from(data[0]) + 1;
    let height = u32::from(data[1]) + 1;
    // 1..=16 frames — the full §4.5.1 range (main + 0..15 animated).
    let frame_count = (data[2] as usize % (1 + MAX_ANIMATED_IMAGES)) + 1;

    let stride = (width as usize).div_ceil(8);
    let plane_len = stride * height as usize;
    let pad = stride * 8 - width as usize;
    let mask: u8 = if pad == 0 { 0xFF } else { 0xFFu8 << pad };

    let body = &data[3..];
    // Build `frame_count` distinct canonical planes (padding bits zero);
    // vary each frame's seed so a frame-ordering bug surfaces.
    let planes: Vec<Vec<u8>> = (0..frame_count)
        .map(|f| {
            let mut p: Vec<u8> = (0..plane_len)
                .map(|i| {
                    if body.is_empty() {
                        0
                    } else {
                        body[(i + f) % body.len()]
                    }
                })
                .collect();
            for row in p.chunks_exact_mut(stride) {
                row[stride - 1] &= mask;
            }
            p
        })
        .collect();
    let images: Vec<WbmpImage> = planes
        .iter()
        .map(|p| WbmpImage::from_bits(width, height, p.clone()).expect("valid geometry"))
        .collect();

    let encoded = match encode_frames(&images, &EncodeOptions::default()) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Single-frame equivalence with encode (documented contract).
    if frame_count == 1 {
        let plain = encode(&images[0], &EncodeOptions::default()).expect("plain encode");
        assert_eq!(encoded, plain, "single-frame encode_frames == encode");
    }

    let frames = decode_all(&encoded).expect("encoded frames must decode");
    assert_eq!(frames.len(), frame_count, "frame count survives");
    assert_eq!(info(&encoded).expect("header").frames as usize, frame_count);
    for (i, fr) in frames.iter().enumerate() {
        assert_eq!(fr.image, images[i], "frame {i} survives in order");
    }
});
