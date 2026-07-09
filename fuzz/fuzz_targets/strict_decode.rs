#![no_main]

//! Drive `parse_wbmp_strict` / `parse_wbmp_strict_with_limits` — the
//! fully-conformant whole-image decode path — over arbitrary bytes.
//!
//! The `decode` target drives the *lax* `parse_wbmp`; `header_ext_strict`
//! drives `parse_header_ext_strict`, which deliberately tolerates a
//! presence-bit-set `FixHeaderField` (it is the extension-aware path). No
//! existing target drives `parse_wbmp_strict`, which is the opposite
//! posture: it routes through `parse_header_strict` (the `FixHeaderField`
//! MUST be `0x00`, no ExtFields) and reads every dimension MBI with the
//! §4.3.1 shortest-encoding check, then performs the full pixel-body
//! length check + row copy.
//!
//! Contract under test: the call always *returns* (no panic / debug
//! overflow / out-of-bounds slice), and the strict acceptance set is a
//! subset of the lax one — anything `parse_wbmp_strict` accepts,
//! `parse_wbmp` must accept and decode to a byte-identical image (strict
//! only ever *adds* rejections on top of lax; it never changes the
//! decoded pixels of a stream both accept).
//!
//! The crate is pulled in with `default-features = false`, so this build
//! never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{parse_wbmp, parse_wbmp_strict, parse_wbmp_strict_with_limits, WbmpLimits};

fuzz_target!(|data: &[u8]| {
    let strict = parse_wbmp_strict(data);

    if let Ok(simg) = &strict {
        // strict ⊆ lax: both use the default limits, so a strict-accepted
        // stream must also decode through the lax parser, identically.
        let limg = parse_wbmp(data).expect("strict-accepted stream must decode lax");
        assert_eq!(simg.width, limg.width, "width agrees strict vs lax");
        assert_eq!(simg.height, limg.height, "height agrees strict vs lax");
        assert_eq!(simg.pixel_format, limg.pixel_format, "polarity agrees");
        assert_eq!(simg.planes.len(), 1, "one plane");
        assert_eq!(
            simg.planes[0].data, limg.planes[0].data,
            "plane bytes agree strict vs lax",
        );
        assert_eq!(simg.planes[0].stride, limg.planes[0].stride, "stride agrees");
        // Plane length is exactly stride * height.
        let expected = simg.planes[0].stride * simg.height as usize;
        assert_eq!(simg.planes[0].data.len(), expected, "plane length");
    }

    // The explicit-limits entry point must also merely return; drive it
    // with the default limits so its acceptance matches parse_wbmp_strict.
    let with_limits = parse_wbmp_strict_with_limits(data, &WbmpLimits::default());
    assert_eq!(
        with_limits.is_ok(),
        strict.is_ok(),
        "strict_with_limits(default) tracks parse_wbmp_strict",
    );
});
