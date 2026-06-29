//! Integration round-trip tests for the WBMP **general-form** (§4.4.1)
//! extension-header surface, exercised through the public crate API as a
//! downstream consumer would use it — `encode_wbmp_ext` /
//! `parse_wbmp_ext`, `parse_header_ext` / `parse_header_ext_strict`,
//! `write_ext_fields` / `write_ext_fields_strict`, and the
//! `Parameter` validating constructor.
//!
//! These complement the per-module unit tests by validating the same
//! behaviour across the crate boundary (only `pub` items are reachable
//! here), which is where a missing re-export or a signature regression
//! would surface. They run on the default-feature build; the standalone
//! build is covered by `--no-default-features --lib`.

use oxideav_wbmp::{
    encode_wbmp, encode_wbmp_ext, parse_ext_fields_strict, parse_header_ext,
    parse_header_ext_strict, parse_wbmp, parse_wbmp_ext, write_ext_fields_strict, ExtFieldType,
    ExtFields, FixHeaderField, Parameter,
};

/// A 16×2 plane (stride 2, 4 packed bytes) used by several cases.
fn plane_16x2() -> Vec<u8> {
    vec![0xF0, 0x0F, 0xAA, 0x55]
}

#[test]
fn encode_ext_none_equals_plain_and_parses_both_ways() {
    let bits = plane_16x2();
    let plain = encode_wbmp(16, 2, &bits).unwrap();
    let ext = encode_wbmp_ext(16, 2, &bits, None, false).unwrap();
    assert_eq!(plain, ext, "None ExtFields == encode_wbmp output");

    // Plain decode path.
    let img = parse_wbmp(&ext).unwrap();
    assert_eq!(img.planes[0].data, bits);

    // Extension-aware decode path agrees and reports no ExtFields.
    let imgx = parse_wbmp_ext(&ext).unwrap();
    assert_eq!(imgx.image.planes[0].data, bits);
    assert!(imgx.ext_fields.is_none());
}

#[test]
fn encode_ext_type11_roundtrips_through_public_api() {
    let bits = plane_16x2();
    let region = ExtFields::ParameterPairs11(vec![
        Parameter::new("model", "X100").unwrap(),
        Parameter::new("rev", "2a").unwrap(),
    ]);
    let encoded = encode_wbmp_ext(16, 2, &bits, Some(&region), true).unwrap();

    let decoded = parse_wbmp_ext(&encoded).unwrap();
    assert_eq!(decoded.image.width, 16);
    assert_eq!(decoded.image.height, 2);
    assert_eq!(decoded.image.planes[0].data, bits);
    assert_eq!(decoded.ext_fields, Some(region));

    // The header-only strict parser must land on the real dimensions too.
    let hdr = parse_header_ext_strict(&encoded).unwrap();
    assert_eq!(hdr.width, 16);
    assert_eq!(hdr.height, 2);
    assert!(hdr.fix_header.ext_fields_follow);
    assert_eq!(hdr.fix_header.ext_type, ExtFieldType::ParameterPairs11);

    // Accessors on the recovered parameters.
    if let Some(ExtFields::ParameterPairs11(pairs)) = &decoded.ext_fields {
        assert_eq!(pairs[0].identifier_str(), Some("model"));
        assert_eq!(pairs[0].value_str(), Some("X100"));
    } else {
        panic!("expected Type-11 ExtFields");
    }
}

#[test]
fn strict_header_ext_rejects_out_of_class_value() {
    // Build a non-conformant Type-11 value byte ('-') via the lax writer,
    // then confirm the strict header parser rejects it while the lax one
    // accepts and decodes it.
    let bits = plane_16x2();
    let bad = ExtFields::ParameterPairs11(vec![Parameter {
        identifier: b"k".to_vec(),
        value: b"a-b".to_vec(),
    }]);
    // Lax writer can emit it.
    let encoded = encode_wbmp_ext(16, 2, &bits, Some(&bad), false).unwrap();

    // Lax header parse accepts.
    let lax = parse_header_ext(&encoded).unwrap();
    assert_eq!(lax.width, 16);
    assert_eq!(lax.ext_fields, Some(bad.clone()));

    // Strict header parse rejects.
    assert!(parse_header_ext_strict(&encoded).is_err());

    // And the strict writer would have refused to emit it in the first
    // place.
    assert!(encode_wbmp_ext(16, 2, &bits, Some(&bad), true).is_err());
}

#[test]
fn strict_ext_fields_write_parse_roundtrip_via_public_api() {
    // Drive the standalone strict ext-field reader/writer pair directly
    // (no full WBMP wrapper) across the crate boundary.
    let region = ExtFields::ParameterPairs11(vec![
        Parameter::new("a", "1").unwrap(),
        Parameter::new("longid7", "value1234567ABC").unwrap(), // 7 / 15 max
    ]);
    let mut buf = Vec::new();
    write_ext_fields_strict(&region, &mut buf).unwrap();

    // The FixHeaderField the writer's caller is expected to emit: presence
    // flag + type 11.
    let fh = FixHeaderField::from_byte(0b1110_0000);
    let mut offset = 0usize;
    let parsed = parse_ext_fields_strict(fh, &buf, &mut offset).unwrap();
    assert_eq!(parsed, Some(region));
    assert_eq!(
        offset,
        buf.len(),
        "strict re-parse consumes exactly what was written"
    );
}

#[test]
fn parameter_new_enforces_abnf_at_the_boundary() {
    // Valid.
    assert!(Parameter::new("Name", "Val123").is_ok());
    // Non-ALPHA/DIGIT value.
    assert!(Parameter::new("k", "a_b").is_err());
    // Empty identifier.
    assert!(Parameter::new("", "1").is_err());
    // Over-long identifier (8 > 7 representable).
    assert!(Parameter::new("eightlen", "1").is_err());
    // Over-long value (16 > 15 representable).
    assert!(Parameter::new("k", "0123456789abcdef").is_err());
}
