#![no_main]

//! Drive arbitrary fuzz-supplied bytes through `parse_header_ext` — the
//! general-form WBMP header parser (WAP-237 §4.4.1–§4.4.3) that decodes
//! the `FixHeaderField` bitfields and, when its bit-7 presence flag is
//! set, the variable-length `ExtFields` region before reading the
//! `Width`/`Height` MBIs.
//!
//! The other targets exercise `parse_wbmp` (which uses the plain
//! four-field `parse_header`, treating the FixHeaderField as opaque) and
//! the encoder paths; none of them reach the extension-header machinery.
//! That machinery has the most attacker-driven control flow in the
//! crate: a 2-bit type selector picking between a continuation-bit
//! bitfield chain (type 00), two single-octet reserved forms (types 01 /
//! 10), and a parameter/value-pair chain with attacker-chosen per-pair
//! identifier/value byte counts (type 11) — each followed by two more
//! multi-byte-integer dimension parses. The classic crash spots are the
//! `*offset` advance arithmetic across each variant, the
//! `MAX_EXT_FIELD_BYTES` chain caps, and the dimension MBIs whose offset
//! now starts after a fuzz-controlled-length ExtFields region.
//!
//! Contract under test: `parse_header_ext` must always *return* a
//! `Result` — a malformed stream yields `Err(WbmpError::…)`, a
//! well-formed one yields `Ok(HeaderExt)`, and neither path may panic,
//! integer-overflow (debug build), index out of bounds, or read past the
//! input slice. When a parse succeeds with an `ExtFields` region
//! present, the parse→`write_ext_fields`→`parse_ext_fields` invariant is
//! additionally asserted: re-serialising the decoded region and re-
//! parsing it must reproduce the same `ExtFields` and consume exactly the
//! bytes written.
//!
//! The crate is pulled in with `default-features = false`, so this build
//! exercises the framework-free standalone path and never links
//! `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{parse_ext_fields, parse_header_ext, write_ext_fields};

fuzz_target!(|data: &[u8]| {
    let Ok(header) = parse_header_ext(data) else {
        return;
    };

    // A successful parse must report dimensions the spec guarantees are
    // non-zero (parse_header_ext rejects a zero width/height) and a
    // data_offset that lands within or at the end of the input — never
    // past it, since every field was read from `data`.
    assert!(header.width >= 1, "decoded width is at least 1");
    assert!(header.height >= 1, "decoded height is at least 1");
    assert!(
        header.data_offset <= data.len(),
        "data_offset {} stays within the {}-byte input",
        header.data_offset,
        data.len()
    );

    // The FixHeaderField presence flag and the parsed ExtFields option
    // must agree: ext_fields is Some iff the bit-7 flag was set.
    assert_eq!(
        header.fix_header.ext_fields_follow,
        header.ext_fields.is_some(),
        "ExtFields presence matches the FixHeaderField bit-7 flag"
    );

    // When an ExtFields region decoded, re-serialise it and re-parse the
    // emitted bytes: the round trip must reproduce the same region and
    // consume exactly what was written. (write_ext_fields can legitimately
    // reject a parsed-but-not-writable region — e.g. an empty type-00
    // chain is unreachable from a successful parse, but a type-11 chain
    // whose parsed sizes exceed the writer's tighter 1..=7 / 1..=15 bounds
    // could be — so a write error is not a fault; only a mismatching
    // re-parse is.)
    if let Some(ext) = header.ext_fields {
        let mut buf = Vec::new();
        if write_ext_fields(&ext, &mut buf).is_ok() {
            let mut offset = 0usize;
            let reparsed = parse_ext_fields(header.fix_header, &buf, &mut offset)
                .expect("written ExtFields must re-parse");
            assert_eq!(
                reparsed,
                Some(ext),
                "ExtFields survives write/re-parse round trip"
            );
            assert_eq!(
                offset,
                buf.len(),
                "re-parse consumes exactly the written bytes"
            );
        }
    }
});
