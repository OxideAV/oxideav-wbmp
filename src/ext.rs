//! WBMP extension-header (`ExtFields`) parsing — WAP-237 §4.4.1–§4.4.3.
//!
//! The general WBMP header format defined in §4.4.1 is
//!
//! ```text
//!   Header = TypeField FixHeaderField [ExtFields] Width Height
//! ```
//!
//! The single `FixHeaderField` byte (§4.4.2, Table 4-3) carries a
//! presence flag plus the extension-header *type*:
//!
//! ```text
//!   bit 7  (MSB) : ExtFields-follow flag. 1 → one or more ExtFields
//!                  follow the FixHeaderField; 0 → none.
//!   bits 6-5     : extension-header type — 00 / 01 / 10 / 11.
//!   bits 4-0     : reserved.
//! ```
//!
//! When the presence flag is set, the bytes between the FixHeaderField
//! and the `Width` MBI form the `ExtFields` region. Its layout depends
//! on the type selected by bits 6-5 (§4.4.1 bullet list + §4.4.3):
//!
//! * **Type 00** — a multi-byte bitfield. Bit 7 of each octet is a
//!   "more data follows" continuation flag; the other bits are reserved
//!   for future use. (`*ExtFieldType00` in the BNF.)
//! * **Type 01** — reserved for future use. A single octet.
//! * **Type 10** — reserved for future use. A single octet.
//! * **Type 11** — a sequence of parameter/value pairs (§4.4.3,
//!   Table 4-4). Each pair starts with a `ParameterHeader` octet:
//!   bit 7 is a "more pairs follow" concatenation flag, bits 6-4 give
//!   the `ParameterIdentifier` size (1-8 bytes), bits 3-0 give the
//!   `ParameterValue` size (1-16 bytes). The identifier is a
//!   US-ASCII string, the value an alphanumeric string. (`*ExtFieldType11`.)
//!
//! WBMP **Type 0** (the only normatively defined image type)
//! additionally fixes the FixHeaderField at `0x00` — "Extension headers
//! MUST NOT be presented in this format" (§4.5.1, Table 4-6). So in a
//! conformant Type-0 file there are never any ExtFields. This module
//! parses the *general* header form regardless, so a decoder can
//! correctly locate `Width`/`Height` when a producer emits a
//! non-conformant Type-0 file carrying extension headers (rather than
//! mis-reading the first ExtField byte as the width MBI), and so the
//! parsed parameter pairs are available to callers that want them.

use crate::error::{Result, WbmpError};

/// The extension-header *type* carried in `FixHeaderField` bits 6-5
/// (§4.4.2, Table 4-3). The same two bits drive how the `ExtFields`
/// region is laid out (§4.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtFieldType {
    /// Type 00 — a multi-byte bitfield. Each octet's bit 7 is a
    /// "more data follows" continuation flag; the remaining bits are
    /// reserved for future use.
    Bitfield00,
    /// Type 01 — reserved for future use (a single octet).
    Reserved01,
    /// Type 10 — reserved for future use (a single octet).
    Reserved10,
    /// Type 11 — a sequence of parameter/value pairs (§4.4.3).
    ParameterPairs11,
}

impl ExtFieldType {
    /// Decode the 2-bit type field from `FixHeaderField` bits 6-5.
    ///
    /// Only the low two bits of `bits65` are considered; callers pass
    /// `(fix_header >> 5) & 0b11`.
    fn from_bits(bits65: u8) -> ExtFieldType {
        match bits65 & 0b11 {
            0b00 => ExtFieldType::Bitfield00,
            0b01 => ExtFieldType::Reserved01,
            0b10 => ExtFieldType::Reserved10,
            _ => ExtFieldType::ParameterPairs11,
        }
    }
}

/// The decoded `FixHeaderField` byte (§4.4.2, Table 4-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixHeaderField {
    /// Raw byte as read from the stream.
    pub raw: u8,
    /// bit 7 — `true` when one or more `ExtFields` follow.
    pub ext_fields_follow: bool,
    /// bits 6-5 — extension-header type.
    pub ext_type: ExtFieldType,
}

impl FixHeaderField {
    /// Split a raw `FixHeaderField` octet into its bitfields.
    pub fn from_byte(raw: u8) -> FixHeaderField {
        FixHeaderField {
            raw,
            ext_fields_follow: (raw & 0x80) != 0,
            ext_type: ExtFieldType::from_bits(raw >> 5),
        }
    }
}

/// One `ExtFieldType11` parameter/value pair (§4.4.3).
///
/// The identifier is a US-ASCII string of 1-8 bytes; the value is an
/// alphanumeric string of 1-16 bytes. Both are stored verbatim as the
/// bytes read from the stream — this module does not transcode or
/// validate the character classes beyond the length bounds the
/// `ParameterHeader` declares, so a caller that needs strict
/// US-ASCII / alphanumeric conformance can apply it on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    /// Parameter identifier bytes (1-8 bytes, US-ASCII per spec).
    pub identifier: Vec<u8>,
    /// Parameter value bytes (1-16 bytes, alphanumeric per spec).
    pub value: Vec<u8>,
}

/// The fully-parsed `ExtFields` region (§4.4.1, §4.4.3).
///
/// The variant matches the type selected by `FixHeaderField` bits 6-5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtFields {
    /// Type 00 — the raw payload bytes of the multi-byte bitfield
    /// chain (continuation flags stripped). Each element holds the
    /// low seven reserved bits of one octet, in stream order.
    Bitfield00(Vec<u8>),
    /// Type 01 — the single reserved octet.
    Reserved01(u8),
    /// Type 10 — the single reserved octet.
    Reserved10(u8),
    /// Type 11 — the decoded parameter/value pairs, in stream order.
    ParameterPairs11(Vec<Parameter>),
}

/// Upper bound on the number of octets the Type-00 bitfield chain or
/// the Type-11 pair sequence will consume before the parser bails.
///
/// WAP-237 sets no normative cap on the length of an ExtFields region,
/// but every shipped WBMP is Type 0 (no extension headers at all), so
/// any real extension-header chain is short. This ceiling keeps a
/// pathological "every octet sets the continuation bit" stream from
/// driving an unbounded read; 4096 octets is generous for any
/// plausible parameter set while still bounding the worst case.
pub const MAX_EXT_FIELD_BYTES: usize = 4096;

/// Parse the `ExtFields` region (§4.4.1) given the already-decoded
/// `FixHeaderField` and the input bytes, starting at `*offset` (which
/// the caller has positioned just past the FixHeaderField byte).
///
/// On success the `*offset` is advanced past the consumed ExtFields and
/// the decoded [`ExtFields`] is returned. When
/// [`FixHeaderField::ext_fields_follow`] is `false` this is a no-op:
/// it returns `Ok(None)` and leaves `*offset` untouched.
///
/// Errors:
/// * [`WbmpError::InvalidData`] on truncation (the stream ends mid-way
///   through a bitfield chain, a `ParameterHeader`, or its declared
///   identifier/value bytes) or when the chain exceeds
///   [`MAX_EXT_FIELD_BYTES`].
pub fn parse_ext_fields(
    fix_header: FixHeaderField,
    bytes: &[u8],
    offset: &mut usize,
) -> Result<Option<ExtFields>> {
    if !fix_header.ext_fields_follow {
        return Ok(None);
    }
    let parsed = match fix_header.ext_type {
        ExtFieldType::Bitfield00 => ExtFields::Bitfield00(parse_bitfield00(bytes, offset)?),
        ExtFieldType::Reserved01 => ExtFields::Reserved01(read_single_octet(bytes, offset, "01")?),
        ExtFieldType::Reserved10 => ExtFields::Reserved10(read_single_octet(bytes, offset, "10")?),
        ExtFieldType::ParameterPairs11 => {
            ExtFields::ParameterPairs11(parse_parameter_pairs(bytes, offset)?)
        }
    };
    Ok(Some(parsed))
}

/// Read the single reserved octet of a Type-01 / Type-10 ExtField.
fn read_single_octet(bytes: &[u8], offset: &mut usize, ty: &str) -> Result<u8> {
    if *offset >= bytes.len() {
        return Err(WbmpError::invalid(format!(
            "WBMP ExtField type {ty}: truncated (no octet present)"
        )));
    }
    let b = bytes[*offset];
    *offset += 1;
    Ok(b)
}

/// Parse a Type-00 multi-byte bitfield chain (§4.4.1, first bullet).
///
/// Each octet's bit 7 is the continuation flag; the low seven bits are
/// reserved payload, collected (with the flag stripped) into the
/// returned `Vec`. The chain ends at the first octet whose bit 7 is
/// clear.
fn parse_bitfield00(bytes: &[u8], offset: &mut usize) -> Result<Vec<u8>> {
    let mut payload = Vec::new();
    loop {
        if *offset >= bytes.len() {
            return Err(WbmpError::invalid(
                "WBMP ExtField type 00: truncated bitfield (continuation bit still set)",
            ));
        }
        if payload.len() >= MAX_EXT_FIELD_BYTES {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 00: bitfield exceeds {MAX_EXT_FIELD_BYTES} octets"
            )));
        }
        let b = bytes[*offset];
        *offset += 1;
        payload.push(b & 0x7F);
        if (b & 0x80) == 0 {
            return Ok(payload);
        }
    }
}

/// Parse a Type-11 parameter/value-pair chain (§4.4.3, Table 4-4).
///
/// Each pair is `ParameterHeader ParameterIdentifier ParameterValue`:
/// the header octet's bit 7 is the "more pairs follow" concatenation
/// flag, bits 6-4 are the identifier size (1-8 bytes), bits 3-0 are the
/// value size (1-16 bytes). The chain ends at the first pair whose
/// header octet has bit 7 clear.
fn parse_parameter_pairs(bytes: &[u8], offset: &mut usize) -> Result<Vec<Parameter>> {
    let mut pairs = Vec::new();
    let start = *offset;
    loop {
        if *offset >= bytes.len() {
            return Err(WbmpError::invalid(
                "WBMP ExtField type 11: truncated (missing ParameterHeader)",
            ));
        }
        // Bound the total ExtFields span by the consumed byte count so a
        // pathological "every header sets the concat flag" chain can't
        // drive an unbounded read.
        if *offset - start >= MAX_EXT_FIELD_BYTES {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 11: pair chain exceeds {MAX_EXT_FIELD_BYTES} octets"
            )));
        }
        let header = bytes[*offset];
        *offset += 1;

        let more = (header & 0x80) != 0;
        // bits 6-4: identifier size, 1-8 bytes (encoded value 0 → 1
        // byte is NOT how the table reads — Table 4-4 example "110 → 6
        // bytes" means the 3-bit field is the literal byte count, and
        // the BNF `1*8CHAR` / `1*16` ranges make 0 illegal).
        let ident_size = ((header >> 4) & 0b111) as usize;
        // bits 3-0: value size, 1-16 bytes.
        let value_size = (header & 0b1111) as usize;

        if ident_size == 0 {
            return Err(WbmpError::invalid(
                "WBMP ExtField type 11: ParameterIdentifier size of 0 (spec requires 1-8 bytes)",
            ));
        }
        if value_size == 0 {
            return Err(WbmpError::invalid(
                "WBMP ExtField type 11: ParameterValue size of 0 (spec requires 1-16 bytes)",
            ));
        }

        let ident = read_n(bytes, offset, ident_size, "ParameterIdentifier")?;
        let value = read_n(bytes, offset, value_size, "ParameterValue")?;
        pairs.push(Parameter {
            identifier: ident,
            value,
        });

        if !more {
            return Ok(pairs);
        }
    }
}

/// Read exactly `n` bytes starting at `*offset`, advancing it. Errors
/// with a `field`-named truncation message if fewer remain.
fn read_n(bytes: &[u8], offset: &mut usize, n: usize, field: &str) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(n)
        .ok_or_else(|| WbmpError::invalid(format!("WBMP ExtField: {field} size overflow")))?;
    if end > bytes.len() {
        return Err(WbmpError::invalid(format!(
            "WBMP ExtField: truncated {field} (need {n} bytes, {} available)",
            bytes.len().saturating_sub(*offset)
        )));
    }
    let out = bytes[*offset..end].to_vec();
    *offset = end;
    Ok(out)
}

/// Serialize an [`ExtFields`] region back to its on-the-wire octets,
/// appending to `out`. The inverse of [`parse_ext_fields`]'s body for a
/// given variant — useful for round-trip testing and for callers that
/// construct extension-header bytes by hand.
///
/// The caller is responsible for emitting a `FixHeaderField` whose
/// bit-7 presence flag is set and whose bits 6-5 select the matching
/// type *before* these bytes; this helper writes only the ExtFields
/// region itself.
///
/// Errors:
/// * [`WbmpError::InvalidData`] if a [`Parameter`] identifier is not
///   1-8 bytes or a value is not 1-16 bytes (the size fields can't
///   encode an out-of-range length), or if a `Bitfield00` payload
///   octet has a bit set above bit 6 (those bits are the continuation
///   flag this helper owns).
pub fn write_ext_fields(ext: &ExtFields, out: &mut Vec<u8>) -> Result<()> {
    match ext {
        ExtFields::Bitfield00(payload) => {
            if payload.is_empty() {
                return Err(WbmpError::invalid(
                    "WBMP ExtField type 00: empty bitfield chain has no terminating octet",
                ));
            }
            let last = payload.len() - 1;
            for (i, &p) in payload.iter().enumerate() {
                if p & 0x80 != 0 {
                    return Err(WbmpError::invalid(
                        "WBMP ExtField type 00: payload octet sets bit 7 (reserved for continuation)",
                    ));
                }
                let cont = if i == last { 0x00 } else { 0x80 };
                out.push(p | cont);
            }
            Ok(())
        }
        ExtFields::Reserved01(b) | ExtFields::Reserved10(b) => {
            out.push(*b);
            Ok(())
        }
        ExtFields::ParameterPairs11(pairs) => {
            if pairs.is_empty() {
                return Err(WbmpError::invalid(
                    "WBMP ExtField type 11: empty pair chain",
                ));
            }
            let last = pairs.len() - 1;
            for (i, p) in pairs.iter().enumerate() {
                if p.identifier.is_empty() || p.identifier.len() > 7 {
                    return Err(WbmpError::invalid(format!(
                        "WBMP ExtField type 11: ParameterIdentifier length {} not in 1..=7",
                        p.identifier.len()
                    )));
                }
                if p.value.is_empty() || p.value.len() > 15 {
                    return Err(WbmpError::invalid(format!(
                        "WBMP ExtField type 11: ParameterValue length {} not in 1..=15",
                        p.value.len()
                    )));
                }
                let more = if i == last { 0x00 } else { 0x80 };
                let header = more | ((p.identifier.len() as u8) << 4) | (p.value.len() as u8);
                out.push(header);
                out.extend_from_slice(&p.identifier);
                out.extend_from_slice(&p.value);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    // The bit-literal groupings below are deliberately field-aligned to
    // the spec's bitfield boundaries (presence flag | type | reserved,
    // or concat flag | ident-size | value-size) rather than uniform
    // nibbles, so they read as the §4.4.2 / §4.4.3 layouts they encode.
    #![allow(clippy::unusual_byte_groupings)]

    use super::*;

    #[test]
    fn fix_header_no_ext() {
        // Conformant Type-0 FixHeaderField is 0x00: no ExtFields.
        let fh = FixHeaderField::from_byte(0x00);
        assert!(!fh.ext_fields_follow);
        assert_eq!(fh.ext_type, ExtFieldType::Bitfield00);
        assert_eq!(fh.raw, 0x00);
    }

    #[test]
    fn fix_header_decodes_all_type_bits() {
        // bit 7 set + each of the four type encodings in bits 6-5.
        assert_eq!(
            FixHeaderField::from_byte(0b1000_0000).ext_type,
            ExtFieldType::Bitfield00
        );
        assert_eq!(
            FixHeaderField::from_byte(0b1010_0000).ext_type,
            ExtFieldType::Reserved01
        );
        assert_eq!(
            FixHeaderField::from_byte(0b1100_0000).ext_type,
            ExtFieldType::Reserved10
        );
        assert_eq!(
            FixHeaderField::from_byte(0b1110_0000).ext_type,
            ExtFieldType::ParameterPairs11
        );
        // Presence flag honoured independently of the type bits.
        assert!(FixHeaderField::from_byte(0b1110_0000).ext_fields_follow);
        assert!(!FixHeaderField::from_byte(0b0110_0000).ext_fields_follow);
    }

    #[test]
    fn no_ext_fields_is_none_and_no_advance() {
        let fh = FixHeaderField::from_byte(0x00);
        let bytes = [0xDE, 0xAD];
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(res, None);
        assert_eq!(offset, 0, "offset must not move when no ExtFields");
    }

    #[test]
    fn bitfield00_single_octet() {
        // FixHeaderField: bit7=1 (ext follow), type=00. One bitfield
        // octet with bit7 clear → chain ends immediately.
        let fh = FixHeaderField::from_byte(0b1000_0000);
        let bytes = [0x42]; // bit7 clear → final; payload 0x42
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(res, Some(ExtFields::Bitfield00(vec![0x42])));
        assert_eq!(offset, 1);
    }

    #[test]
    fn bitfield00_multi_octet_chain() {
        // Two octets with continuation, one terminating. Payloads have
        // their bit7 stripped.
        let fh = FixHeaderField::from_byte(0b1000_0000);
        // 0x81 → cont + payload 0x01; 0xC2 → cont + payload 0x42;
        // 0x33 → final + payload 0x33.
        let bytes = [0x81, 0xC2, 0x33];
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(res, Some(ExtFields::Bitfield00(vec![0x01, 0x42, 0x33])));
        assert_eq!(offset, 3);
    }

    #[test]
    fn bitfield00_truncated_errors() {
        // Continuation bit set but stream ends.
        let fh = FixHeaderField::from_byte(0b1000_0000);
        let bytes = [0x80];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn bitfield00_chain_cap_errors() {
        // A run of continuation octets longer than MAX_EXT_FIELD_BYTES
        // must error rather than read unbounded.
        let fh = FixHeaderField::from_byte(0b1000_0000);
        let bytes = vec![0x80u8; MAX_EXT_FIELD_BYTES + 8];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        assert!(offset <= MAX_EXT_FIELD_BYTES + 1);
    }

    #[test]
    fn reserved01_reads_one_octet() {
        let fh = FixHeaderField::from_byte(0b1010_0000);
        let bytes = [0xAB, 0xCD];
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(res, Some(ExtFields::Reserved01(0xAB)));
        assert_eq!(offset, 1, "only the one reserved octet is consumed");
    }

    #[test]
    fn reserved10_reads_one_octet() {
        let fh = FixHeaderField::from_byte(0b1100_0000);
        let bytes = [0x7E];
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(res, Some(ExtFields::Reserved10(0x7E)));
        assert_eq!(offset, 1);
    }

    #[test]
    fn reserved_type_truncated_errors() {
        let fh = FixHeaderField::from_byte(0b1010_0000);
        let bytes: [u8; 0] = [];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_single_pair() {
        // Type 11: one pair, ident size 3 ("abc"), value size 2 ("01").
        // ParameterHeader: bit7=0 (no more), bits6-4=011 (3), bits3-0=0010 (2).
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_011_0010u8;
        let mut bytes = vec![header];
        bytes.extend_from_slice(b"abc");
        bytes.extend_from_slice(b"01");
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(
            res,
            Some(ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"abc".to_vec(),
                value: b"01".to_vec(),
            }]))
        );
        assert_eq!(offset, bytes.len());
    }

    #[test]
    fn parameter_pairs_two_pairs_concat_flag() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        // Pair 1: more=1, ident size 1, value size 1.
        let h1 = 0b1_001_0001u8;
        // Pair 2: more=0, ident size 2, value size 3.
        let h2 = 0b0_010_0011u8;
        let mut bytes = vec![h1];
        bytes.extend_from_slice(b"X"); // ident 1
        bytes.extend_from_slice(b"7"); // value 1
        bytes.push(h2);
        bytes.extend_from_slice(b"WH"); // ident 2
        bytes.extend_from_slice(b"abc"); // value 3
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(
            res,
            Some(ExtFields::ParameterPairs11(vec![
                Parameter {
                    identifier: b"X".to_vec(),
                    value: b"7".to_vec(),
                },
                Parameter {
                    identifier: b"WH".to_vec(),
                    value: b"abc".to_vec(),
                },
            ]))
        );
        assert_eq!(offset, bytes.len());
    }

    #[test]
    fn parameter_pairs_max_sizes() {
        // ident size 7 max from a 3-bit field; value size 15 max from a
        // 4-bit field. (Spec ranges are 1-8 / 1-16 but the 3-bit field
        // only encodes up to 7 and the 4-bit field up to 15; the
        // largest representable byte counts are exercised here.)
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_111_1111u8; // more=0, ident 7, value 15
        let mut bytes = vec![header];
        bytes.extend_from_slice(b"IDENT77"); // 7 ident bytes
        bytes.extend_from_slice(b"VALUE1234567890"); // 15 value bytes
        let mut offset = 0;
        let res = parse_ext_fields(fh, &bytes, &mut offset).unwrap();
        assert_eq!(
            res,
            Some(ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"IDENT77".to_vec(),
                value: b"VALUE1234567890".to_vec(),
            }]))
        );
        assert_eq!(offset, bytes.len());
    }

    #[test]
    fn parameter_pairs_zero_ident_size_errors() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_000_0010u8; // ident size 0 → illegal
        let bytes = [header, b'a', b'b'];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_zero_value_size_errors() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_001_0000u8; // value size 0 → illegal
        let bytes = [header, b'a'];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_truncated_identifier_errors() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_100_0001u8; // ident size 4, value size 1
        let bytes = [header, b'a', b'b']; // only 2 of 4 ident bytes
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_truncated_value_errors() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let header = 0b0_001_0011u8; // ident 1, value 3
        let bytes = [header, b'I', b'v']; // ident ok, value short (1/3)
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_missing_header_errors() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let bytes: [u8; 0] = [];
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    // --- write_ext_fields round-trip coverage. ---

    fn roundtrip(fix_byte: u8, ext: ExtFields) {
        let fh = FixHeaderField::from_byte(fix_byte);
        let mut buf = Vec::new();
        write_ext_fields(&ext, &mut buf).unwrap();
        let mut offset = 0;
        let parsed = parse_ext_fields(fh, &buf, &mut offset).unwrap();
        assert_eq!(parsed, Some(ext));
        assert_eq!(offset, buf.len(), "parse consumes exactly what write emits");
    }

    #[test]
    fn roundtrip_bitfield00() {
        roundtrip(0b1000_0000, ExtFields::Bitfield00(vec![0x01]));
        roundtrip(0b1000_0000, ExtFields::Bitfield00(vec![0x7F, 0x00, 0x42]));
    }

    #[test]
    fn roundtrip_reserved01_10() {
        roundtrip(0b1010_0000, ExtFields::Reserved01(0x5A));
        roundtrip(0b1100_0000, ExtFields::Reserved10(0xA5));
    }

    #[test]
    fn roundtrip_parameter_pairs() {
        roundtrip(
            0b1110_0000,
            ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"name".to_vec(),
                value: b"v1".to_vec(),
            }]),
        );
        roundtrip(
            0b1110_0000,
            ExtFields::ParameterPairs11(vec![
                Parameter {
                    identifier: b"a".to_vec(),
                    value: b"1".to_vec(),
                },
                Parameter {
                    identifier: b"longid7".to_vec(),
                    value: b"value1234567890".to_vec(), // 15 bytes
                },
            ]),
        );
    }

    #[test]
    fn write_rejects_bitfield00_payload_high_bit() {
        let mut buf = Vec::new();
        let err = write_ext_fields(&ExtFields::Bitfield00(vec![0x80]), &mut buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn write_rejects_empty_bitfield00() {
        let mut buf = Vec::new();
        let err = write_ext_fields(&ExtFields::Bitfield00(vec![]), &mut buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn write_rejects_oversize_identifier() {
        let mut buf = Vec::new();
        let err = write_ext_fields(
            &ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"toolongid".to_vec(), // 9 > 7
                value: b"v".to_vec(),
            }]),
            &mut buf,
        )
        .unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn write_rejects_oversize_value() {
        let mut buf = Vec::new();
        let err = write_ext_fields(
            &ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"i".to_vec(),
                value: b"sixteen_byte_val".to_vec(), // 16 > 15
            }]),
            &mut buf,
        )
        .unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_pairs_runaway_concat_caps() {
        // Every ParameterHeader sets the concat flag with the smallest
        // ident/value sizes (1/1), so each pair consumes 3 bytes. A
        // long run with no terminating header must hit the byte cap.
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let mut bytes = Vec::new();
        for _ in 0..(MAX_EXT_FIELD_BYTES / 3 + 4) {
            bytes.push(0b1_001_0001u8); // more=1, ident 1, value 1
            bytes.push(b'i');
            bytes.push(b'v');
        }
        let mut offset = 0;
        let err = parse_ext_fields(fh, &bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }
}
