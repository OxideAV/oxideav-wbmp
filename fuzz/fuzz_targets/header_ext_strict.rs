#![no_main]

//! Drive arbitrary fuzz-supplied bytes through `parse_header_ext_strict`
//! — the fully-conformant general-form WBMP header parser (WAP-237
//! §4.4.1–§4.4.3) that, on top of the offset arithmetic the lax
//! `header_ext` target already exercises, enforces two normative
//! tightenings: (a) every Multi-Byte Integer (Type, Width, Height) must
//! use the §4.3.1 shortest encoding (no redundant leading `0x80`), and
//! (b) a Type-11 `ExtFields` region's `ParameterIdentifier` bytes must be
//! US-ASCII `CHAR` (`%x01-7F`) and its `ParameterValue` bytes must be
//! `ALPHA / DIGIT` (`A-Za-z0-9`) per the §4.4.3 / §4.2 ABNF.
//!
//! This is the only target reaching the strict character-class state
//! machine in `parse_ext_fields_strict` and the strict-MBI gating on the
//! extension-aware path. The invariants asserted express that **strict is
//! a refinement of lax**:
//!
//!   1. The call always *returns* a `Result` — never panics, debug-
//!      overflows, indexes out of bounds, or reads past the input slice.
//!   2. Anything strict *accepts*, lax also accepts and decodes
//!      identically (strict only ever rejects, never changes the decode).
//!   3. A strict-accepted Type-11 region's every parameter satisfies the
//!      character classes (`Parameter::validate` agrees), so the strict
//!      reader and the `Parameter::new` constructor are consistent.
//!   4. A strict-accepted region survives a `write_ext_fields_strict` →
//!      `parse_ext_fields_strict` round trip byte-for-byte.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! exercises the framework-free standalone path and never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    parse_ext_fields_strict, parse_header_ext, parse_header_ext_strict, write_ext_fields_strict,
    ExtFields,
};

fuzz_target!(|data: &[u8]| {
    let Ok(strict) = parse_header_ext_strict(data) else {
        return;
    };

    // (1) basic well-formedness of the accepted header.
    assert!(strict.width >= 1, "decoded width is at least 1");
    assert!(strict.height >= 1, "decoded height is at least 1");
    assert!(
        strict.data_offset <= data.len(),
        "data_offset {} stays within the {}-byte input",
        strict.data_offset,
        data.len()
    );
    assert_eq!(
        strict.fix_header.ext_fields_follow,
        strict.ext_fields.is_some(),
        "ExtFields presence matches the FixHeaderField bit-7 flag"
    );

    // (2) strict ⊆ lax: anything strict accepts, lax accepts and decodes
    // to exactly the same HeaderExt. (The strict path only ever tightens
    // acceptance; it never changes the bytes that come out.)
    let lax = parse_header_ext(data).expect("lax accepts everything strict accepts");
    assert_eq!(lax, strict, "strict decode matches lax decode");

    // (3) a strict-accepted Type-11 region's parameters are all in-class,
    // so Parameter::validate agrees with the reader that accepted them.
    if let Some(ExtFields::ParameterPairs11(pairs)) = &strict.ext_fields {
        for p in pairs {
            p.validate()
                .expect("strict-accepted parameter must satisfy its own validator");
            // The identifier/value accessors must succeed too: a
            // spec-conformant identifier (US-ASCII) and value (A-Za-z0-9)
            // are always valid UTF-8.
            assert!(p.identifier_str().is_some(), "in-class identifier is UTF-8");
            assert!(p.value_str().is_some(), "in-class value is UTF-8");
        }
    }

    // (4) strict write → strict re-parse round trip. write_ext_fields_strict
    // may legitimately reject a parsed region whose sizes exceed the
    // writer's 1..=7 / 1..=15 bounds (unreachable for a strict-accepted
    // Type-11 region, but a defensive guard), so a write error is not a
    // fault; only a mismatching re-parse is.
    if let Some(ext) = strict.ext_fields {
        let mut buf = Vec::new();
        if write_ext_fields_strict(&ext, &mut buf).is_ok() {
            let mut offset = 0usize;
            let reparsed = parse_ext_fields_strict(strict.fix_header, &buf, &mut offset)
                .expect("strict-written ExtFields must strict-re-parse");
            assert_eq!(
                reparsed,
                Some(ext),
                "ExtFields survives strict write/re-parse round trip"
            );
            assert_eq!(
                offset,
                buf.len(),
                "strict re-parse consumes exactly the written bytes"
            );
        }
    }
});
