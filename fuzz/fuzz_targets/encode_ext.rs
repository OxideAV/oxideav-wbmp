#![no_main]

//! Round-trip the general-form extension-header *writer* — the only
//! public surface no other target reaches. `encode` + `EncodeOptions::ext_fields` (§4.4.1)
//! synthesises a `TypeField FixHeaderField [ExtFields] Width Height`
//! header (choosing the FixHeaderField type bits from the `ExtFields`
//! variant), appends the packed plane, and is the documented inverse of
//! `decode` / `info`. `header_ext` only round-trips the region-level
//! `write_ext_fields`; `decode_ext` only *reads* arbitrary bytes through
//! `decode` / `info`. Neither closes the encode → decode loop over the
//! full file writer for every `ExtFields` variant.
//!
//! This target synthesises a plane plus one of the four `ExtFields`
//! variants (`None`, `Bitfield00`, `Reserved01`, `Reserved10`,
//! `ParameterPairs11`) from the fuzz bytes, encodes it with both the lax
//! and strict `encode` + `EncodeOptions::ext_fields`, decodes with `decode` / `info`, and
//! asserts:
//!
//!  * the image (width / height / plane bytes) survives byte-for-byte;
//!  * the `ExtFields` survive exactly (Type-00 payload octets are built
//!    masked to their low 7 bits so the continuation-flag strip on
//!    decode reproduces them, and Type-11 parameters are built in-class);
//!  * a `None` ext field makes the output byte-identical to `encode`
//!    (the documented equivalence).
//!
//! The crate is pulled in with `default-features = false`, so this build
//! never links `oxideav-core`.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{decode, encode, info, EncodeOptions, ExtFields, Parameter, WbmpImage};

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
    // Canonical plane (padding bits zero): `encode` normalises the
    // padding, so the round trip is exact on canonical input.
    let pad = stride * 8 - width as usize;
    let mask: u8 = if pad == 0 { 0xFF } else { 0xFFu8 << pad };
    let mut plane: Vec<u8> = (0..plane_len)
        .map(|i| {
            if body.is_empty() {
                0
            } else {
                body[i % body.len()]
            }
        })
        .collect();
    for row in plane.chunks_exact_mut(stride) {
        row[stride - 1] &= mask;
    }
    let image = WbmpImage::from_bits(width, height, plane.clone()).expect("valid geometry");

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
        let opts = EncodeOptions::default()
            .with_ext_fields(ext.clone())
            .with_strict(strict);
        let encoded = match encode(&image, &opts) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let decoded = decode(&encoded).expect("encoded ext stream must decode");
        assert_eq!(decoded, image, "image survives");
        let h = info(&encoded).expect("header");
        assert_eq!(h.ext_fields, ext, "ext fields survive round trip");
        assert_eq!((h.fix_header & 0x80) != 0, ext.is_some());

        if ext.is_none() {
            let plain = encode(&image, &EncodeOptions::default()).expect("plain encode");
            assert_eq!(encoded, plain, "no-ext encode == plain encode");
        }
    }
});
