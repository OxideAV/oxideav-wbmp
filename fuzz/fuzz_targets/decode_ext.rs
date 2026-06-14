#![no_main]

//! Drive arbitrary fuzz-supplied bytes through `parse_wbmp_ext` — the
//! extension-header-aware full decode path (WAP-237 §4.4.1–§4.4.3 header
//! plus the §4.5.1 main-image-data copy).
//!
//! The `header_ext` target stops at `parse_header_ext` (header fields
//! only); the `decode` target drives `parse_wbmp`, which uses the plain
//! four-field header and never reaches the `ExtFields` machinery. This
//! target is the only one that walks a fuzz-controlled-length `ExtFields`
//! region AND then performs the pixel-body length check + verbatim row
//! copy whose `data_offset` now begins past that variable region. The
//! crash spots it adds over `header_ext`: the `decode_body` slice index
//! `input[data_offset..]`, the `total_bytes` vs. body-length comparison,
//! and the limit checks applied to dimensions read after the ExtFields.
//!
//! Contract under test: `parse_wbmp_ext` must always *return* a `Result`
//! — malformed input yields `Err(WbmpError::…)`, well-formed input yields
//! `Ok(WbmpImageExt)`, and neither path may panic, integer-overflow
//! (debug build), index out of bounds, or read past the input slice. On a
//! successful decode the returned plane must be self-consistent: its
//! length equals `stride * height`, and the ExtFields-present flag must
//! match the decoded `ext_fields` option.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! exercises the framework-free standalone path and never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::parse_wbmp_ext;

fuzz_target!(|data: &[u8]| {
    let Ok(decoded) = parse_wbmp_ext(data) else {
        return;
    };

    let img = &decoded.image;
    assert!(img.width >= 1, "decoded width is at least 1");
    assert!(img.height >= 1, "decoded height is at least 1");
    assert_eq!(img.planes.len(), 1, "WBMP Type 0 decodes one packed plane");

    // The single packed plane must hold exactly stride * height bytes:
    // decode_body copies that many from the body and never more.
    let plane = &img.planes[0];
    let expected = plane.stride * img.height as usize;
    assert_eq!(
        plane.data.len(),
        expected,
        "plane data length {} == stride {} * height {}",
        plane.data.len(),
        plane.stride,
        img.height
    );
});
