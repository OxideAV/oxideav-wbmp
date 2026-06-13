//! WBMP Type-0 header parser.
//!
//! WAP-237 defines a single byte-stream header for the only widely-
//! deployed WBMP variant (Type 0, "uncompressed B/W bitmap"):
//!
//! ```text
//!   ┌─────────────┬────────────────┬───────┬────────┐
//!   │ Type (MBI)  │ FixedHeader (1)│ Width │ Height │
//!   │ value = 0   │ value = 0x00   │ MBI   │ MBI    │
//!   └─────────────┴────────────────┴───────┴────────┘
//! ```
//!
//! * `Type` — variable-length unsigned integer ([`crate::mbi`]). Only
//!   value `0` is standardised; later WAP releases reserved further
//!   values for greyscale / colour bitmaps but never published a
//!   normative encoding.
//! * `FixedHeader` — exactly one byte. In Type 0 it is always `0x00`
//!   (the "Ext Headers" bit-7 is unset and the remaining seven bits
//!   are reserved).
//! * `Width`, `Height` — MBI unsigned integers, in pixels.
//!
//! Pixel data follows immediately after the header. Each row is
//! `ceil(width / 8)` bytes, packed MSB-first; bit `1` represents
//! white, bit `0` black (per WAP-237 §8.4 "B/W bitmap" — see
//! [`crate::WbmpImage`] for how the decoded plane lays out).

use crate::error::{Result, WbmpError};
use crate::ext::{parse_ext_fields, ExtFields, FixHeaderField};
use crate::mbi::{read_mbi_u32, write_mbi_u32};

/// Decoded WBMP header — Type 0 only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Byte offset at which pixel data starts (i.e. just past the
    /// parsed header).
    pub data_offset: usize,
}

/// Decoded WBMP header including any parsed extension headers
/// (`ExtFields`, §4.4.1–§4.4.3).
///
/// This is the richer counterpart to [`Header`]: it carries the
/// decoded [`FixHeaderField`] bitfields and, when the FixHeaderField's
/// presence flag is set, the parsed [`ExtFields`] region. Use
/// [`parse_header_ext`] to obtain it.
///
/// In a conformant WBMP **Type 0** file `fix_header.ext_fields_follow`
/// is always `false` and `ext_fields` is `None` — §4.5.1 fixes the
/// FixHeaderField at `0x00` ("Extension headers MUST NOT be presented
/// in this format"). The extension-header machinery exists so the
/// decoder can still correctly locate `Width`/`Height` (and surface the
/// parameters) when a producer emits a non-conformant Type-0 file
/// carrying extension headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderExt {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Byte offset at which pixel data starts (just past the parsed
    /// header, including any ExtFields).
    pub data_offset: usize,
    /// The decoded `FixHeaderField` byte (§4.4.2).
    pub fix_header: FixHeaderField,
    /// The parsed `ExtFields` region, or `None` when the FixHeaderField
    /// presence flag is clear.
    pub ext_fields: Option<ExtFields>,
}

impl HeaderExt {
    /// Narrow to the plain four-field [`Header`] view, discarding the
    /// extension-header detail.
    pub fn header(&self) -> Header {
        Header {
            width: self.width,
            height: self.height,
            data_offset: self.data_offset,
        }
    }
}

/// Parse the four header fields. Returns the [`Header`] on success
/// and the byte offset at which pixel data begins.
///
/// This is the **lax** parser: the `FixedHeader` byte is accepted at
/// any value (the spec text says it is presently unused in Type 0,
/// but the byte is mandatory). For callers that want a strict
/// conformance check — refusing any input whose `FixedHeader` byte is
/// not the spec-mandated `0x00` — see [`parse_header_strict`].
///
/// Errors:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero.
/// * [`WbmpError::InvalidData`] for truncated or oversized MBIs.
pub fn parse_header(bytes: &[u8]) -> Result<Header> {
    parse_header_inner(bytes, false)
}

/// Strict variant of [`parse_header`]. Identical except the
/// `FixedHeader` byte is required to be exactly `0x00` — the value
/// WAP-237 §8 fixes for the only defined Type (0, B/W bitmap). Any
/// other value raises [`WbmpError::InvalidData`].
///
/// Use this entry point when the caller needs to reject malformed /
/// non-conformant Type-0 files at the wire-format level rather than
/// silently accept a byte the spec does not currently assign meaning
/// to. The lax [`parse_header`] is forward-compatible with
/// hypothetical Type-0 extensions; this one is not.
pub fn parse_header_strict(bytes: &[u8]) -> Result<Header> {
    parse_header_inner(bytes, true)
}

/// Parse the header **including** any extension headers (`ExtFields`,
/// §4.4.1–§4.4.3), returning the richer [`HeaderExt`].
///
/// Unlike [`parse_header`] — which treats the `FixHeaderField` byte as
/// opaque and reads `Width`/`Height` immediately after it — this entry
/// point honours the FixHeaderField's bit-7 presence flag: when it is
/// set, the `ExtFields` region (whose layout is selected by bits 6-5)
/// is decoded and skipped before reading `Width`/`Height`. That keeps
/// the decoder from mis-reading the first ExtField octet as the width
/// MBI on a (non-conformant) Type-0 file that carries extension
/// headers, and surfaces the parsed [`ExtFields`] to the caller.
///
/// In a conformant Type-0 file the presence flag is always clear, so
/// `ext_fields` comes back `None` and the result is byte-for-byte
/// equivalent to [`parse_header`].
///
/// Errors:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero.
/// * [`WbmpError::InvalidData`] for truncated/oversized MBIs, a
///   truncated or over-long ExtFields region, or a zero dimension.
pub fn parse_header_ext(bytes: &[u8]) -> Result<HeaderExt> {
    let mut offset = 0usize;

    // Field 1: Type (MBI). Type 0 only.
    let typ = read_mbi_u32(bytes, &mut offset)?;
    if typ != 0 {
        return Err(WbmpError::unsupported(format!(
            "WBMP type {typ} (only type 0 / B/W bitmap is supported)"
        )));
    }

    // Field 2: FixHeaderField (one byte) — decoded into its bitfields.
    if offset >= bytes.len() {
        return Err(WbmpError::invalid("WBMP: header truncated at FixedHeader"));
    }
    let fix_header = FixHeaderField::from_byte(bytes[offset]);
    offset += 1;

    // Field 3 (optional): ExtFields, present iff the FixHeaderField
    // bit-7 presence flag is set.
    let ext_fields = parse_ext_fields(fix_header, bytes, &mut offset)?;

    // Fields 4-5: Width, Height (MBIs).
    let width = read_mbi_u32(bytes, &mut offset)?;
    let height = read_mbi_u32(bytes, &mut offset)?;

    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "WBMP: zero dimension (width={width}, height={height})"
        )));
    }

    Ok(HeaderExt {
        width,
        height,
        data_offset: offset,
        fix_header,
        ext_fields,
    })
}

fn parse_header_inner(bytes: &[u8], strict: bool) -> Result<Header> {
    let mut offset = 0usize;

    // Field 1: Type (MBI). Type 0 only.
    let typ = read_mbi_u32(bytes, &mut offset)?;
    if typ != 0 {
        return Err(WbmpError::unsupported(format!(
            "WBMP type {typ} (only type 0 / B/W bitmap is supported)"
        )));
    }

    // Field 2: FixedHeader. Exactly one byte. The lax path accepts any
    // value (forward-compat with hypothetical Type-0 extensions); the
    // strict path requires the spec-mandated 0x00.
    if offset >= bytes.len() {
        return Err(WbmpError::invalid("WBMP: header truncated at FixedHeader"));
    }
    let fixed_header = bytes[offset];
    if strict && fixed_header != 0x00 {
        return Err(WbmpError::invalid(format!(
            "WBMP: FixedHeader byte = 0x{fixed_header:02X}, strict mode requires 0x00"
        )));
    }
    offset += 1;

    // Fields 3-4: Width, Height (MBIs).
    let width = read_mbi_u32(bytes, &mut offset)?;
    let height = read_mbi_u32(bytes, &mut offset)?;

    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "WBMP: zero dimension (width={width}, height={height})"
        )));
    }

    Ok(Header {
        width,
        height,
        data_offset: offset,
    })
}

/// Append a Type-0 header (Type=0, FixedHeader=0, Width, Height) to
/// `out`. Pixel data must be appended by the caller right after.
pub fn write_header(width: u32, height: u32, out: &mut Vec<u8>) {
    write_mbi_u32(0, out); // Type = 0 (B/W bitmap)
    out.push(0x00); // FixedHeader (Type 0: always 0)
    write_mbi_u32(width, out);
    write_mbi_u32(height, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_small() {
        let mut buf = Vec::new();
        write_header(96, 64, &mut buf);
        // Type=0 → 1 byte; FixedHeader → 1 byte; 96 (=0x60) → 1 byte;
        // 64 (=0x40) → 1 byte. Both dimensions fit in the single-byte
        // MBI range (≤ 0x7F) so the whole header is 4 bytes.
        assert_eq!(buf, [0x00, 0x00, 0x60, 0x40]);
        let header = parse_header(&buf).unwrap();
        assert_eq!(header.width, 96);
        assert_eq!(header.height, 64);
        assert_eq!(header.data_offset, buf.len());
    }

    #[test]
    fn header_roundtrip_large_dimensions() {
        // 1280 × 720 — both dimensions need 2-byte MBIs.
        let mut buf = Vec::new();
        write_header(1280, 720, &mut buf);
        let header = parse_header(&buf).unwrap();
        assert_eq!(header.width, 1280);
        assert_eq!(header.height, 720);
    }

    #[test]
    fn rejects_non_zero_type() {
        // Type = 1 (single-byte MBI).
        let buf = [0x01u8, 0x00, 0x10, 0x10];
        let err = parse_header(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)));
    }

    #[test]
    fn rejects_zero_dimension() {
        // Width = 0, Height = 1.
        let buf = [0x00u8, 0x00, 0x00, 0x01];
        let err = parse_header(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn rejects_truncated_at_fixed_header() {
        // Type=0 only — no FixedHeader / Width / Height bytes.
        let buf = [0x00u8];
        let err = parse_header(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn accepts_arbitrary_fixed_header_byte() {
        // Type 0 + FixedHeader=0xFF + 1×1 image. We tolerate non-zero
        // FixedHeader bytes for forward-compat.
        let buf = [0x00u8, 0xFF, 0x01, 0x01];
        let header = parse_header(&buf).unwrap();
        assert_eq!(header.width, 1);
        assert_eq!(header.height, 1);
        assert_eq!(header.data_offset, 4);
    }

    #[test]
    fn strict_accepts_zero_fixed_header() {
        // The strict parser must still accept conformant Type-0
        // headers (FixedHeader = 0x00) and produce the same result as
        // the lax parser.
        let mut buf = Vec::new();
        write_header(96, 64, &mut buf);
        assert_eq!(buf, [0x00, 0x00, 0x60, 0x40]);
        let lax = parse_header(&buf).unwrap();
        let strict = parse_header_strict(&buf).unwrap();
        assert_eq!(lax, strict);
        assert_eq!(strict.width, 96);
        assert_eq!(strict.height, 64);
        assert_eq!(strict.data_offset, buf.len());
    }

    #[test]
    fn strict_rejects_nonzero_fixed_header_byte() {
        // The lax parser accepts FixedHeader = 0xFF (see
        // accepts_arbitrary_fixed_header_byte). The strict parser must
        // reject it as InvalidData — the spec fixes the byte at 0x00
        // for Type 0.
        let buf = [0x00u8, 0xFF, 0x01, 0x01];
        // Lax: still accepted.
        assert!(parse_header(&buf).is_ok());
        // Strict: rejected.
        let err = parse_header_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        if let WbmpError::InvalidData(msg) = &err {
            assert!(
                msg.contains("0xFF") && msg.contains("strict"),
                "message should name the offending byte and the mode: {msg}"
            );
        }
    }

    #[test]
    fn strict_rejects_high_bit_only_fixed_header() {
        // The high bit of FixedHeader is reserved in the spec text as
        // the "Ext Headers" indicator (no normative encoding ever
        // published). A header carrying just that bit must still be
        // rejected by the strict parser.
        let buf = [0x00u8, 0x80, 0x01, 0x01];
        let err = parse_header_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn strict_still_rejects_nonzero_type() {
        // Non-zero Type field must surface as Unsupported in both
        // parsers — the strict mode tightens the FixedHeader check,
        // not the Type check.
        let buf = [0x01u8, 0x00, 0x10, 0x10];
        let err = parse_header_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn strict_still_rejects_zero_dimension() {
        // Zero width/height must still be InvalidData in strict mode.
        let buf = [0x00u8, 0x00, 0x00, 0x01];
        let err = parse_header_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    // --- parse_header_ext (extension-header aware) tests. ---

    #[test]
    fn ext_conformant_type0_matches_plain_header() {
        // FixHeaderField = 0x00 (presence flag clear): parse_header_ext
        // must agree with parse_header and report no ExtFields.
        let mut buf = Vec::new();
        write_header(96, 64, &mut buf);
        let plain = parse_header(&buf).unwrap();
        let ext = parse_header_ext(&buf).unwrap();
        assert_eq!(ext.header(), plain);
        assert!(ext.ext_fields.is_none());
        assert!(!ext.fix_header.ext_fields_follow);
        assert_eq!(ext.width, 96);
        assert_eq!(ext.height, 64);
        assert_eq!(ext.data_offset, buf.len());
    }

    #[test]
    fn ext_skips_type11_parameter_pairs_before_dimensions() {
        // Non-conformant Type-0 file carrying a single Type-11
        // parameter pair. The plain parser would mis-read the
        // ParameterHeader octet as the width MBI; parse_header_ext must
        // skip the ExtFields and land on the real Width/Height.
        use crate::ext::{write_ext_fields, ExtFields, Parameter};
        let mut buf = Vec::new();
        // Type = 0.
        write_mbi_u32(0, &mut buf);
        // FixHeaderField: bit7=1 (ext follow), type=11.
        buf.push(0b1110_0000);
        // One parameter pair.
        let ext = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"x".to_vec(),
            value: b"1".to_vec(),
        }]);
        write_ext_fields(&ext, &mut buf).unwrap();
        // Width = 200 (2-byte MBI), Height = 3.
        write_mbi_u32(200, &mut buf);
        write_mbi_u32(3, &mut buf);

        let parsed = parse_header_ext(&buf).unwrap();
        assert_eq!(parsed.width, 200);
        assert_eq!(parsed.height, 3);
        assert_eq!(parsed.ext_fields, Some(ext));
        assert_eq!(parsed.data_offset, buf.len());
        assert!(parsed.fix_header.ext_fields_follow);
    }

    #[test]
    fn ext_skips_bitfield00_chain_before_dimensions() {
        use crate::ext::{write_ext_fields, ExtFields};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf);
        buf.push(0b1000_0000); // bit7=1, type=00
        let ext = ExtFields::Bitfield00(vec![0x01, 0x42]);
        write_ext_fields(&ext, &mut buf).unwrap();
        write_mbi_u32(8, &mut buf); // width
        write_mbi_u32(8, &mut buf); // height
        let parsed = parse_header_ext(&buf).unwrap();
        assert_eq!(parsed.width, 8);
        assert_eq!(parsed.height, 8);
        assert_eq!(parsed.ext_fields, Some(ext));
    }

    #[test]
    fn ext_rejects_nonzero_type() {
        let buf = [0x01u8, 0x00, 0x10, 0x10];
        let err = parse_header_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn ext_rejects_truncated_ext_fields() {
        // Presence flag set, type=00, but the bitfield chain never
        // terminates before the stream ends.
        let buf = [0x00u8, 0b1000_0000, 0x80];
        let err = parse_header_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn ext_rejects_zero_dimension_after_ext() {
        use crate::ext::{write_ext_fields, ExtFields};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf);
        buf.push(0b1000_0000);
        write_ext_fields(&ExtFields::Bitfield00(vec![0x00]), &mut buf).unwrap();
        write_mbi_u32(0, &mut buf); // width = 0 → invalid
        write_mbi_u32(1, &mut buf);
        let err = parse_header_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn strict_still_rejects_truncated_at_fixed_header() {
        // Truncation before the FixedHeader byte must still surface as
        // InvalidData (truncation), not as a strict-mode rejection
        // (since the byte isn't present).
        let buf = [0x00u8];
        let err = parse_header_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        if let WbmpError::InvalidData(msg) = &err {
            // The truncation message — not the strict-mode rejection
            // message — should fire.
            assert!(msg.contains("truncated"), "{msg}");
        }
    }
}
