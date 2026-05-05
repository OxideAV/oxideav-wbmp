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

/// Parse the four header fields. Returns the [`Header`] on success
/// and the byte offset at which pixel data begins.
///
/// Errors:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero.
/// * [`WbmpError::InvalidData`] for truncated or oversized MBIs.
pub fn parse_header(bytes: &[u8]) -> Result<Header> {
    let mut offset = 0usize;

    // Field 1: Type (MBI). Type 0 only.
    let typ = read_mbi_u32(bytes, &mut offset)?;
    if typ != 0 {
        return Err(WbmpError::unsupported(format!(
            "WBMP type {typ} (only type 0 / B/W bitmap is supported)"
        )));
    }

    // Field 2: FixedHeader. Exactly one byte. We accept any value
    // here for Type 0 — the WAP-237 spec text is "Type 0 currently
    // does not use the FixedHeader field" but the byte is still
    // mandatory in the wire format. Treating it as opaque keeps us
    // forward-compatible with hypothetical Type-0 extensions.
    if offset >= bytes.len() {
        return Err(WbmpError::invalid("WBMP: header truncated at FixedHeader"));
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
}
