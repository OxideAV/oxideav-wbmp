#![no_main]

//! Exercise the Multi-Byte Integer (MBI) codec directly — the
//! `write_mbi_u32` / `read_mbi_u32` / `read_mbi_u32_strict` / `mbi_u32_len`
//! quartet WAP-237 §4.3.1 defines and every WBMP header field is built
//! from. The other targets reach the MBI codec only transitively through
//! a whole-header parse, so none of them drives the *writer* across the
//! full `u32` value space or asserts the encoder/decoder invariants in
//! isolation. This target does both.
//!
//! Two halves share the fuzz input:
//!
//!  1. **Writer round trip** — take a `u32` from the first four bytes,
//!     encode it with `write_mbi_u32`, and assert the §4.3.1 invariants
//!     hold: the encoded length equals `mbi_u32_len` and is 1..=5 octets,
//!     the first octet is never `0x80` (the shortest-encoding MUST NOT),
//!     every non-final octet sets its continuation bit and the final one
//!     clears it, and both `read_mbi_u32` and `read_mbi_u32_strict`
//!     recover the exact value while consuming exactly the emitted bytes.
//!
//!  2. **Arbitrary decode** — feed the raw fuzz bytes to both readers and
//!     assert neither panics / overflows (debug) / reads past the slice.
//!     The strict reader must accept a subset of what the lax reader
//!     accepts and agree on the value + consumption when it does; any lax
//!     success must consume 1..=len bytes and re-encode to a form no
//!     longer than the octets it consumed (padding only ever *adds*
//!     octets over the minimal form).
//!
//! The crate is pulled in with `default-features = false`, so this build
//! never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    mbi_u32_len, read_mbi_u32, read_mbi_u32_strict, write_mbi_u32, MAX_U32_MBI_BYTES,
};

fuzz_target!(|data: &[u8]| {
    // --- Half 1: writer round-trips every u32 value.
    if data.len() >= 4 {
        let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let mut buf = Vec::new();
        write_mbi_u32(value, &mut buf);

        // Length agrees with the size estimator and stays within the
        // 5-octet u32 worst case.
        assert_eq!(buf.len(), mbi_u32_len(value), "mbi_u32_len == encoded length");
        assert!(
            (1..=MAX_U32_MBI_BYTES).contains(&buf.len()),
            "encoded length {} out of 1..={MAX_U32_MBI_BYTES}",
            buf.len(),
        );

        // §4.3.1 shortest encoding: the first octet is never 0x80.
        assert_ne!(buf[0], 0x80, "writer emitted a leading 0x80 for {value:#x}");

        // Continuation-bit discipline: bit 7 set on every octet except
        // the last, clear on the last.
        let last = buf.len() - 1;
        for (i, &b) in buf.iter().enumerate() {
            assert_eq!(
                (b & 0x80) != 0,
                i != last,
                "continuation flag wrong at octet {i} for {value:#x}",
            );
        }

        // Lax reader recovers the value and consumes exactly the bytes.
        let mut off = 0;
        let got = read_mbi_u32(&buf, &mut off).expect("writer output must decode");
        assert_eq!(got, value, "lax decode round trip");
        assert_eq!(off, buf.len(), "lax decode consumes all emitted bytes");

        // The minimal encoding is by definition strict-conformant.
        let mut soff = 0;
        let sgot =
            read_mbi_u32_strict(&buf, &mut soff).expect("minimal encoding is strict-valid");
        assert_eq!(sgot, value, "strict decode round trip");
        assert_eq!(soff, buf.len(), "strict decode consumes all emitted bytes");
    }

    // --- Half 2: arbitrary bytes through both readers never panic.
    let mut off_lax = 0;
    let lax = read_mbi_u32(data, &mut off_lax);
    let mut off_strict = 0;
    let strict = read_mbi_u32_strict(data, &mut off_strict);

    // Strict ⊆ lax: whatever strict accepts, lax accepts with the same
    // value and the same consumption.
    if let Ok(sv) = strict {
        let lv = *lax.as_ref().expect("strict-accepted MBI must also decode lax");
        assert_eq!(sv, lv, "strict/lax value agree");
        assert_eq!(off_strict, off_lax, "strict/lax consumption agree");
    }

    // On any lax success: consumption stays in-bounds and the decoded
    // value's minimal encoding is no longer than the octets consumed
    // (leading-0x80 padding only ever adds octets).
    if let Ok(lv) = lax {
        assert!(off_lax >= 1 && off_lax <= data.len(), "consumption in bounds");
        assert!(
            mbi_u32_len(lv) <= off_lax,
            "minimal length {} exceeds consumed {off_lax}",
            mbi_u32_len(lv),
        );
    }
});
