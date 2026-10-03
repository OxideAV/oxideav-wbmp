#![no_main]

//! Drive the strict Type-0 decode (`decode_with` + `strict = true`: the
//! `FixHeaderField` MUST be `0x00`, every MBI in §4.3.1 shortest form)
//! over arbitrary bytes, alongside the lenient `decode`.
//!
//! Contract under test: the call always *returns* (no panic / debug
//! overflow / out-of-bounds slice), and the strict acceptance set is a
//! subset of the lenient one — anything strict accepts, lenient must
//! accept and decode to an identical image (strict only ever *adds*
//! rejections; it never changes the decoded pixels of a stream both
//! accept). A strict-accepted stream also has `FixHeaderField == 0x00`
//! and no extension headers as reported by `info`.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{decode, decode_with, info, DecodeOptions};

fuzz_target!(|data: &[u8]| {
    let strict_opts = DecodeOptions::default().with_strict(true);
    let strict = decode_with(data, &strict_opts);

    if let Ok(simg) = &strict {
        let limg = decode(data).expect("strict-accepted stream must decode leniently");
        assert_eq!(*simg, limg, "strict ⊆ lenient, identical image");
        assert_eq!(simg.planes.len(), 1, "one plane");
        let expected = simg.planes[0].stride * simg.height as usize;
        assert_eq!(simg.planes[0].data.len(), expected, "plane length");
        let h = info(data).expect("header parses");
        assert_eq!(h.fix_header, 0x00, "strict ⇒ conformant FixHeaderField");
        assert!(h.ext_fields.is_none());
    }

    // A second strict call with an explicit (default) byte cap tracks
    // the first: the options are pure data.
    let again = decode_with(
        data,
        &strict_opts
            .clone()
            .with_max_bytes(DecodeOptions::DEFAULT_MAX_BYTES),
    );
    assert_eq!(again.is_ok(), strict.is_ok());
});
