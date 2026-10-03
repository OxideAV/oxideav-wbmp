//! Integration round-trip tests for the WBMP **general-form** (§4.4.1)
//! extension-header surface, exercised through the public crate API as a
//! downstream consumer would use it — `encode` with
//! `EncodeOptions::ext_fields`, `info` (`ImageInfo::ext_fields`), the
//! lenient / strict `decode_with`, `write_ext_fields_strict` /
//! `parse_ext_fields_strict`, and the `Parameter` validating
//! constructor.
//!
//! These complement the per-module unit tests by validating the same
//! behaviour across the crate boundary (only `pub` items are reachable
//! here), which is where a missing re-export or a signature regression
//! would surface.

use oxideav_wbmp::{
    decode, decode_with, encode, info, parse_ext_fields_strict, write_ext_fields_strict,
    DecodeOptions, EncodeOptions, Error, ExtFieldType, ExtFields, FixHeaderField, Parameter,
    WbmpImage,
};

/// A 16×2 image (stride 2, 4 packed bytes) used by several cases.
fn image_16x2() -> WbmpImage {
    WbmpImage::from_bits(16, 2, vec![0xF0, 0x0F, 0xAA, 0x55]).unwrap()
}

#[test]
fn encode_ext_none_equals_plain_and_parses_both_ways() {
    let img = image_16x2();
    let plain = encode(&img, &EncodeOptions::default()).unwrap();
    let ext = encode(&img, &EncodeOptions::default().with_ext_fields(None)).unwrap();
    assert_eq!(plain, ext, "None ExtFields == plain output");

    assert_eq!(decode(&ext).unwrap(), img);
    assert_eq!(
        decode_with(&ext, &DecodeOptions::default().with_strict(true)).unwrap(),
        img
    );
    let i = info(&ext).unwrap();
    assert_eq!(i.fix_header, 0x00);
    assert!(i.ext_fields.is_none());
}

#[test]
fn encode_ext_type11_roundtrips_through_public_api() {
    let img = image_16x2();
    let region = ExtFields::ParameterPairs11(vec![
        Parameter::new("model", "X100").unwrap(),
        Parameter::new("rev", "2a").unwrap(),
    ]);
    let encoded = encode(
        &img,
        &EncodeOptions::default()
            .with_ext_fields(region.clone())
            .with_strict(true),
    )
    .unwrap();

    // Lenient decode lands on the real dimensions.
    assert_eq!(decode(&encoded).unwrap(), img);
    let i = info(&encoded).unwrap();
    assert_eq!((i.width, i.height), (16, 2));
    assert_eq!(i.ext_fields, Some(region));
    let fh = FixHeaderField::from_byte(i.fix_header);
    assert!(fh.ext_fields_follow);
    assert_eq!(fh.ext_type, ExtFieldType::ParameterPairs11);
    assert_eq!(i.data_offset + 4, encoded.len());

    // Accessors on the recovered parameters.
    if let Some(ExtFields::ParameterPairs11(pairs)) = &i.ext_fields {
        assert_eq!(pairs[0].identifier_str(), Some("model"));
        assert_eq!(pairs[0].value_str(), Some("X100"));
    } else {
        panic!("expected Type-11 ExtFields");
    }

    // Type 0 forbids extension headers (§4.5.1): strict decode refuses.
    assert!(matches!(
        decode_with(&encoded, &DecodeOptions::default().with_strict(true)),
        Err(Error::InvalidData(_))
    ));
}

#[test]
fn strict_writer_and_strict_reader_reject_out_of_class_value() {
    // Build a non-conformant Type-11 value byte ('-') via the lax
    // writer, then confirm the strict region parser rejects it while
    // the lax decode accepts it.
    let img = image_16x2();
    let bad = ExtFields::ParameterPairs11(vec![Parameter {
        identifier: b"k".to_vec(),
        value: b"a-b".to_vec(),
    }]);
    let encoded = encode(&img, &EncodeOptions::default().with_ext_fields(bad.clone())).unwrap();

    // Lax decode accepts and surfaces the region.
    assert_eq!(decode(&encoded).unwrap(), img);
    assert_eq!(info(&encoded).unwrap().ext_fields, Some(bad.clone()));

    // Strict region parse (just past the FixHeaderField) rejects.
    let fh = FixHeaderField::from_byte(encoded[1]);
    let mut offset = 2usize;
    assert!(parse_ext_fields_strict(fh, &encoded, &mut offset).is_err());

    // And the strict writer refuses to emit it in the first place.
    assert!(matches!(
        encode(
            &img,
            &EncodeOptions::default()
                .with_ext_fields(bad)
                .with_strict(true)
        ),
        Err(Error::InvalidData(_))
    ));
}

#[test]
fn strict_ext_fields_write_parse_roundtrip_via_public_api() {
    let region = ExtFields::ParameterPairs11(vec![
        Parameter::new("a", "1").unwrap(),
        Parameter::new("longid7", "value1234567ABC").unwrap(), // 7 / 15 max
    ]);
    let mut buf = Vec::new();
    write_ext_fields_strict(&region, &mut buf).unwrap();

    let fh = FixHeaderField::from_byte(0b1110_0000);
    let mut offset = 0usize;
    let parsed = parse_ext_fields_strict(fh, &buf, &mut offset).unwrap();
    assert_eq!(parsed, Some(region));
    assert_eq!(offset, buf.len());
}

#[test]
fn parameter_new_enforces_abnf_at_the_boundary() {
    assert!(Parameter::new("Name", "Val123").is_ok());
    assert!(Parameter::new("k", "a_b").is_err());
    assert!(Parameter::new("", "1").is_err());
    assert!(Parameter::new("eightlen", "1").is_err());
    assert!(Parameter::new("k", "0123456789abcdef").is_err());
}
