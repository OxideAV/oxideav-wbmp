#![no_main]

//! Round-trip the general-form extension-header *writer* — the only
//! public surface no other target reaches. `encode_wbmp_ext` (§4.4.1)
//! synthesises a `TypeField FixHeaderField [ExtFields] Width Height`
//! header (choosing the FixHeaderField type bits from the `ExtFields`
//! variant), appends the packed plane, and is the documented inverse of
//! `parse_wbmp_ext`. `header_ext` only round-trips the region-level
//! `write_ext_fields`; `decode_ext` only *reads* arbitrary bytes through
//! `parse_wbmp_ext`. Neither closes the encode → decode loop over the
//! full file writer for every `ExtFields` variant.
//!
//! This target synthesises a plane plus one of the four `ExtFields`
//! variants (`None`, `Bitfield00`, `Reserved01`, `Reserved10`,
//! `ParameterPairs11`) from the fuzz bytes, encodes it with both the lax
//! and strict `encode_wbmp_ext`, decodes with `parse_wbmp_ext`, and
//! asserts:
//!
//!  * the image (width / height / plane bytes) survives byte-for-byte;
//!  * the `ExtFields` survive exactly (Type-00 payload octets are built
//!    masked to their low 7 bits so the continuation-flag strip on
//!    decode reproduces them, and Type-11 parameters are built in-class);
//!  * a `None` ext field makes the output byte-identical to `encode_wbmp`
//!    (the documented equivalence).
//!
//! The crate is pulled in with `default-features = false`, so this build
//! never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    encode_wbmp, encode_wbmp_ext, parse_wbmp_ext, ExtFields, Parameter,
};

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    // Small in-bounds dimensions (1..=256) keep the plane well under the
    // default limits so a valid encode always decodes.
    let width = u32::from(data[0]) + 1;
    let height = u32::from(data[1]) + 1;
    let selector = data[2];
    let body = &data[3..];

    let stride = (width as usize).div_ceil(8);
    let plane_len = stride * height as usize;
    // Verbatim plane; encode_wbmp_ext / parse_wbmp_ext copy the body
    // unchanged, so any byte pattern round-trips regardless of padding.
    let plane: Vec<u8> = (0..plane_len)
        .map(|i| if body.is_empty() { 0 } else { body[i % body.len()] })
        .collect();

    // Build one of the five ext-field shapes from the selector.
    let ext: Option<ExtFields> = match selector % 5 {
        0 => None,
        1 => {
            // 1..=8 payload octets, each masked to the low 7 reserved bits.
            let n = 1 + (selector as usize >> 3) % 8;
            let payload: Vec<u8> = (0..n)
                .map(|k| body.get(k).copied().unwrap_or(0) & 0x7F)
                .collect();
            Some(ExtFields::Bitfield00(payload))
        }
        2 => Some(ExtFields::Reserved01(body.first().copied().unwrap_or(0))),
        3 => Some(ExtFields::Reserved10(body.first().copied().unwrap_or(0))),
        _ => {
            // 1..=3 in-class parameter/value pairs.
            let count = 1 + (selector as usize >> 3) % 3;
            let mut pairs = Vec::new();
            for k in 0..count {
                let id_len = 1 + (body.get(2 * k).copied().unwrap_or(0) as usize % 7);
                let val_len = 1 + (body.get(2 * k + 1).copied().unwrap_or(1) as usize % 15);
                let id: Vec<u8> = vec![b'a'; id_len];
                let val: Vec<u8> = vec![b'0'; val_len];
                if let Ok(p) = Parameter::new(id, val) {
                    pairs.push(p);
                }
            }
            if pairs.is_empty() {
                None
            } else {
                Some(ExtFields::ParameterPairs11(pairs))
            }
        }
    };

    // Both the lax and strict writers must round-trip these in-class ext
    // fields identically.
    for &strict in &[false, true] {
        let encoded = match encode_wbmp_ext(width, height, &plane, ext.as_ref(), strict) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let decoded = parse_wbmp_ext(&encoded).expect("encoded ext stream must decode");
        assert_eq!(decoded.image.width, width, "width survives");
        assert_eq!(decoded.image.height, height, "height survives");
        assert_eq!(decoded.image.planes[0].data, plane, "plane survives");
        assert_eq!(decoded.ext_fields, ext, "ext fields survive round trip");

        if ext.is_none() {
            let plain = encode_wbmp(width, height, &plane).expect("plain encode");
            assert_eq!(encoded, plain, "no-ext encode == encode_wbmp");
        }
    }
});
