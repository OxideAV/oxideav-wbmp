#![no_main]

//! Drive arbitrary fuzz-supplied bytes through the lenient `decode` +
//! `info` pair — the extension-header-aware path (WAP-237 §4.4.1–§4.4.3
//! header plus the §4.5.1 main-image-data copy).
//!
//! `header_ext` stops at the header; this target walks a
//! fuzz-controlled-length `ExtFields` region AND then performs the
//! pixel-body length check + verbatim row copy whose `data_offset`
//! begins past that variable region. The crash spots it adds: the
//! body slice at `data_offset`, the `total_bytes` vs. body-length
//! comparison, and the limit checks applied to dimensions read after
//! the ExtFields.
//!
//! Contract under test: both calls always *return*; on a successful
//! decode the plane is self-consistent (`stride × height` bytes, wire
//! bytes verbatim), `info` agrees on the geometry, and the
//! `FixHeaderField` presence bit matches `ImageInfo::ext_fields`.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! exercises the framework-free standalone path and never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{decode, info};

fuzz_target!(|data: &[u8]| {
    let Ok(img) = decode(data) else {
        return;
    };
    let h = info(data).expect("decode ⇒ info");

    assert!(img.width >= 1, "decoded width is at least 1");
    assert!(img.height >= 1, "decoded height is at least 1");
    assert_eq!((img.width, img.height), (h.width, h.height));
    assert_eq!(img.planes.len(), 1, "WBMP Type 0 decodes one packed plane");
    assert_eq!((h.fix_header & 0x80) != 0, h.ext_fields.is_some());

    let plane = &img.planes[0];
    let expected = plane.stride * img.height as usize;
    assert_eq!(plane.data.len(), expected, "plane data length");
    assert_eq!(
        &plane.data[..],
        &data[h.data_offset..h.data_offset + expected],
        "plane is the wire bytes past the (possibly extended) header"
    );
});
