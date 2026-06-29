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
/// bytes read from the stream — the **lax** parser
/// ([`parse_ext_fields`]) does not transcode or validate the character
/// classes beyond the length bounds the `ParameterHeader` declares, so a
/// caller that needs strict US-ASCII / alphanumeric conformance reaches
/// for the strict parser ([`parse_ext_fields_strict`]) or validates a
/// constructed [`Parameter`] with [`Parameter::new`] / [`Parameter::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    /// Parameter identifier bytes (1-8 bytes, US-ASCII per spec).
    pub identifier: Vec<u8>,
    /// Parameter value bytes (1-16 bytes, alphanumeric per spec).
    pub value: Vec<u8>,
}

/// `true` when `b` is a US-ASCII `CHAR` per [RFC 2234] §6.1 — the
/// character class WAP-237 §4.4.3 names for `ParameterIdentifier`
/// (`1*8CHAR`). RFC 2234 defines `CHAR = %x01-7F`: any 7-bit ASCII
/// character except the NUL octet.
///
/// [RFC 2234]: https://www.rfc-editor.org/rfc/rfc2234
#[inline]
fn is_rfc2234_char(b: u8) -> bool {
    (0x01..=0x7F).contains(&b)
}

/// `true` when `b` is an `ALPHA / DIGIT` per [RFC 2234] §6.1 — the
/// character class WAP-237 §4.4.3 names for `ParameterValue`
/// (`1*16(ALPHA / DIGIT)`). RFC 2234 defines `ALPHA = %x41-5A / %x61-7A`
/// (`A-Z` / `a-z`) and `DIGIT = %x30-39` (`0-9`).
///
/// [RFC 2234]: https://www.rfc-editor.org/rfc/rfc2234
#[inline]
fn is_rfc2234_alpha_digit(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

impl Parameter {
    /// Construct a [`Parameter`], validating it against the normative
    /// WAP-237 §4.4.3 / §4.2 ABNF in one step:
    ///
    /// * the `ParameterIdentifier` must be 1-7 bytes of US-ASCII `CHAR`
    ///   (`%x01-7F`); and
    /// * the `ParameterValue` must be 1-15 bytes of `ALPHA / DIGIT`
    ///   (`A-Za-z0-9`).
    ///
    /// The upper length bounds are 7 / 15 (not the ABNF's 8 / 16) because
    /// the `ParameterHeader` size fields are only 3 / 4 bits wide and so
    /// can encode at most a literal byte count of 7 / 15 — a length of
    /// 8 / 16 has no on-wire representation in this format. See
    /// [`write_ext_fields`] for the same bound on the writer side.
    ///
    /// Errors with [`WbmpError::InvalidData`] naming the first offending
    /// constraint.
    pub fn new(identifier: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Result<Parameter> {
        let p = Parameter {
            identifier: identifier.into(),
            value: value.into(),
        };
        p.validate()?;
        Ok(p)
    }

    /// Validate this [`Parameter`] against the normative WAP-237 §4.4.3 /
    /// §4.2 ABNF — same checks [`Parameter::new`] performs. Returns
    /// `Ok(())` when the identifier is 1-7 bytes of US-ASCII `CHAR` and
    /// the value is 1-15 bytes of `ALPHA / DIGIT`, else
    /// [`WbmpError::InvalidData`].
    pub fn validate(&self) -> Result<()> {
        if self.identifier.is_empty() || self.identifier.len() > 7 {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 11: ParameterIdentifier length {} not in 1..=7",
                self.identifier.len()
            )));
        }
        if let Some(&bad) = self.identifier.iter().find(|&&b| !is_rfc2234_char(b)) {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 11: ParameterIdentifier byte 0x{bad:02X} is not US-ASCII \
                 CHAR (%x01-7F) per §4.4.3"
            )));
        }
        if self.value.is_empty() || self.value.len() > 15 {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 11: ParameterValue length {} not in 1..=15",
                self.value.len()
            )));
        }
        if let Some(&bad) = self.value.iter().find(|&&b| !is_rfc2234_alpha_digit(b)) {
            return Err(WbmpError::invalid(format!(
                "WBMP ExtField type 11: ParameterValue byte 0x{bad:02X} is not ALPHA / DIGIT \
                 (A-Za-z0-9) per §4.4.3"
            )));
        }
        Ok(())
    }

    /// The identifier as a `&str` when it is valid UTF-8 (it always is
    /// for a spec-conformant identifier, whose bytes are US-ASCII).
    /// Returns `None` if the stored bytes are not valid UTF-8.
    pub fn identifier_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.identifier).ok()
    }

    /// The value as a `&str` when it is valid UTF-8 (it always is for a
    /// spec-conformant value, whose bytes are `A-Za-z0-9`). Returns
    /// `None` if the stored bytes are not valid UTF-8.
    pub fn value_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.value).ok()
    }
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
    parse_ext_fields_inner(fix_header, bytes, offset, false)
}

/// Strict variant of [`parse_ext_fields`]. Identical except that, for a
/// Type-11 parameter-pair region, every `ParameterIdentifier` byte must
/// be a US-ASCII `CHAR` (`%x01-7F`) and every `ParameterValue` byte must
/// be `ALPHA / DIGIT` (`A-Za-z0-9`) — the normative character classes
/// WAP-237 §4.4.3 / §4.2 names (RFC 2234 ABNF). A byte outside those
/// classes raises [`WbmpError::InvalidData`].
///
/// The lax [`parse_ext_fields`] stores the bytes verbatim regardless of
/// their character class (forward-compat / tolerant decode); this entry
/// point rejects an ABNF-violating stream. The Type-00 / Type-01 /
/// Type-10 regions carry opaque reserved octets with no character-class
/// constraint, so for those types the two parsers behave identically.
pub fn parse_ext_fields_strict(
    fix_header: FixHeaderField,
    bytes: &[u8],
    offset: &mut usize,
) -> Result<Option<ExtFields>> {
    parse_ext_fields_inner(fix_header, bytes, offset, true)
}

fn parse_ext_fields_inner(
    fix_header: FixHeaderField,
    bytes: &[u8],
    offset: &mut usize,
    strict: bool,
) -> Result<Option<ExtFields>> {
    if !fix_header.ext_fields_follow {
        return Ok(None);
    }
    let parsed = match fix_header.ext_type {
        ExtFieldType::Bitfield00 => ExtFields::Bitfield00(parse_bitfield00(bytes, offset)?),
        ExtFieldType::Reserved01 => ExtFields::Reserved01(read_single_octet(bytes, offset, "01")?),
        ExtFieldType::Reserved10 => ExtFields::Reserved10(read_single_octet(bytes, offset, "10")?),
        ExtFieldType::ParameterPairs11 => {
            ExtFields::ParameterPairs11(parse_parameter_pairs(bytes, offset, strict)?)
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
fn parse_parameter_pairs(bytes: &[u8], offset: &mut usize, strict: bool) -> Result<Vec<Parameter>> {
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
        let param = Parameter {
            identifier: ident,
            value,
        };
        if strict {
            // Length is already bounded by the 3-/4-bit size fields; this
            // adds the §4.4.3 character-class constraint (identifier =
            // US-ASCII CHAR, value = ALPHA / DIGIT).
            param.validate()?;
        }
        pairs.push(param);

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
    write_ext_fields_inner(ext, out, false)
}

/// Strict variant of [`write_ext_fields`]. Identical except that each
/// Type-11 [`Parameter`] is additionally validated against the normative
/// character classes (identifier = US-ASCII `CHAR`, value =
/// `ALPHA / DIGIT`) before it is emitted, so a strict writer can never
/// produce an ABNF-violating stream. A parameter whose bytes fall
/// outside those classes raises [`WbmpError::InvalidData`].
///
/// The plain [`write_ext_fields`] enforces only the length bounds (the
/// size fields physically can't encode an out-of-range length); this one
/// also enforces the character classes, mirroring the
/// [`parse_ext_fields_strict`] read side.
pub fn write_ext_fields_strict(ext: &ExtFields, out: &mut Vec<u8>) -> Result<()> {
    write_ext_fields_inner(ext, out, true)
}

fn write_ext_fields_inner(ext: &ExtFields, out: &mut Vec<u8>, strict: bool) -> Result<()> {
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
                if strict {
                    // Adds the §4.4.3 character-class check on top of the
                    // length bounds enforced unconditionally below.
                    p.validate()?;
                }
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

    // --- Strict parameter character-class validation (§4.4.3 ABNF). ---

    /// Build a Type-11 ExtFields buffer holding one pair with the given
    /// raw identifier / value bytes (caller picks lengths within the
    /// 1..=7 / 1..=15 representable ranges).
    fn one_pair_buf(ident: &[u8], value: &[u8]) -> Vec<u8> {
        let header = ((ident.len() as u8) << 4) | (value.len() as u8); // more=0
        let mut buf = vec![header];
        buf.extend_from_slice(ident);
        buf.extend_from_slice(value);
        buf
    }

    #[test]
    fn strict_accepts_conformant_pair() {
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let buf = one_pair_buf(b"Name", b"Val123");
        let mut o_lax = 0;
        let mut o_strict = 0;
        let lax = parse_ext_fields(fh, &buf, &mut o_lax).unwrap();
        let strict = parse_ext_fields_strict(fh, &buf, &mut o_strict).unwrap();
        assert_eq!(lax, strict, "conformant pair parses identically");
        assert_eq!(o_lax, o_strict);
        assert_eq!(
            strict,
            Some(ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"Name".to_vec(),
                value: b"Val123".to_vec(),
            }]))
        );
    }

    #[test]
    fn strict_rejects_non_ascii_identifier_byte() {
        // 0x80 is outside RFC 2234 CHAR (%x01-7F). The lax parser keeps
        // it verbatim; the strict parser rejects it.
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let buf = one_pair_buf(&[b'a', 0x80], b"1");
        // Lax: accepted, byte stored verbatim.
        let mut o = 0;
        let lax = parse_ext_fields(fh, &buf, &mut o).unwrap();
        assert_eq!(
            lax,
            Some(ExtFields::ParameterPairs11(vec![Parameter {
                identifier: vec![b'a', 0x80],
                value: b"1".to_vec(),
            }]))
        );
        // Strict: rejected, message names the offending byte + class.
        let mut o2 = 0;
        let err = parse_ext_fields_strict(fh, &buf, &mut o2).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        if let WbmpError::InvalidData(msg) = &err {
            assert!(msg.contains("0x80") && msg.contains("CHAR"), "{msg}");
        }
    }

    #[test]
    fn strict_rejects_nul_identifier_byte() {
        // RFC 2234 CHAR explicitly excludes the NUL octet (%x00).
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let buf = one_pair_buf(&[0x00], b"1");
        let mut o = 0;
        let err = parse_ext_fields_strict(fh, &buf, &mut o).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn strict_rejects_non_alnum_value_byte() {
        // A hyphen is a valid ParameterValue byte in many grammars but
        // WAP-237 restricts the value to ALPHA / DIGIT only.
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let buf = one_pair_buf(b"k", b"a-b");
        // Lax: accepted.
        let mut o = 0;
        assert!(parse_ext_fields(fh, &buf, &mut o).is_ok());
        // Strict: rejected, message names the byte + class.
        let mut o2 = 0;
        let err = parse_ext_fields_strict(fh, &buf, &mut o2).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        if let WbmpError::InvalidData(msg) = &err {
            assert!(msg.contains("ALPHA") && msg.contains("DIGIT"), "{msg}");
        }
    }

    #[test]
    fn strict_accepts_value_with_letters_and_digits() {
        // Mixed case + digits is the full ALPHA / DIGIT class.
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let buf = one_pair_buf(b"K", b"aZ09");
        let mut o = 0;
        let res = parse_ext_fields_strict(fh, &buf, &mut o).unwrap();
        assert_eq!(
            res,
            Some(ExtFields::ParameterPairs11(vec![Parameter {
                identifier: b"K".to_vec(),
                value: b"aZ09".to_vec(),
            }]))
        );
    }

    #[test]
    fn strict_validates_every_pair_in_a_chain() {
        // First pair conformant, second pair has a bad value byte. The
        // strict parser must reject the chain, not stop after the first.
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let h1 = 0b1_001_0001u8; // more=1, ident 1, value 1
        let h2 = 0b0_001_0001u8; // more=0, ident 1, value 1
        let mut buf = vec![h1];
        buf.extend_from_slice(b"a"); // ident
        buf.extend_from_slice(b"1"); // value (ok)
        buf.push(h2);
        buf.extend_from_slice(b"b"); // ident
        buf.push(b'!'); // value '!' — not ALPHA / DIGIT
        let mut o = 0;
        let err = parse_ext_fields_strict(fh, &buf, &mut o).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parameter_new_validates_and_accessors() {
        let p = Parameter::new(b"name".to_vec(), b"Val1".to_vec()).unwrap();
        assert_eq!(p.identifier_str(), Some("name"));
        assert_eq!(p.value_str(), Some("Val1"));
    }

    #[test]
    fn parameter_new_rejects_bad_classes_and_lengths() {
        // Non-ALPHA/DIGIT value.
        assert!(Parameter::new(b"k".to_vec(), b"a_b".to_vec()).is_err());
        // Non-CHAR identifier (0x80).
        assert!(Parameter::new(vec![0x80], b"1".to_vec()).is_err());
        // Empty identifier.
        assert!(Parameter::new(Vec::new(), b"1".to_vec()).is_err());
        // Identifier too long (8 > 7).
        assert!(Parameter::new(b"eightlen".to_vec(), b"1".to_vec()).is_err());
        // Value too long (16 > 15).
        assert!(Parameter::new(b"k".to_vec(), b"0123456789abcdef".to_vec()).is_err());
    }

    #[test]
    fn write_strict_rejects_non_conformant_pair() {
        let mut buf = Vec::new();
        // Bypass Parameter::new's validation by constructing directly.
        let bad = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"k".to_vec(),
            value: vec![b'a', 0x01], // 0x01 is a CHAR but not ALPHA/DIGIT
        }]);
        // Lax writer emits it (only length-checked).
        assert!(write_ext_fields(&bad, &mut buf).is_ok());
        // Strict writer rejects it.
        let mut buf2 = Vec::new();
        let err = write_ext_fields_strict(&bad, &mut buf2).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn write_strict_roundtrips_conformant() {
        let ext = ExtFields::ParameterPairs11(vec![
            Parameter::new(b"a".to_vec(), b"1".to_vec()).unwrap(),
            Parameter::new(b"longid7".to_vec(), b"value1234567ABC".to_vec()).unwrap(),
        ]);
        let mut buf = Vec::new();
        write_ext_fields_strict(&ext, &mut buf).unwrap();
        let fh = FixHeaderField::from_byte(0b1110_0000);
        let mut o = 0;
        let parsed = parse_ext_fields_strict(fh, &buf, &mut o).unwrap();
        assert_eq!(parsed, Some(ext));
        assert_eq!(o, buf.len());
    }

    #[test]
    fn strict_type00_and_reserved_match_lax() {
        // Non-Type-11 regions have no character-class constraint, so the
        // strict and lax parsers must agree byte-for-byte.
        for (fix, region) in [
            (0b1000_0000u8, ExtFields::Bitfield00(vec![0x01, 0x42])),
            (0b1010_0000u8, ExtFields::Reserved01(0x5A)),
            (0b1100_0000u8, ExtFields::Reserved10(0xA5)),
        ] {
            let mut buf = Vec::new();
            write_ext_fields(&region, &mut buf).unwrap();
            let fh = FixHeaderField::from_byte(fix);
            let mut ol = 0;
            let mut os = 0;
            let lax = parse_ext_fields(fh, &buf, &mut ol).unwrap();
            let strict = parse_ext_fields_strict(fh, &buf, &mut os).unwrap();
            assert_eq!(lax, strict);
            assert_eq!(ol, os);
        }
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
