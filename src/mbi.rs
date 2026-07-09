//! Multi-Byte Integer (MBI) codec used throughout WBMP headers.
//!
//! WAP-237 defines MBI as the same variable-length unsigned-int
//! encoding WAP and WSP use everywhere: payload bits are big-endian,
//! seven bits per byte; the most-significant bit of each byte is the
//! "continuation" flag — 1 means "more bytes follow", 0 means "this
//! byte holds the trailing 7 bits of the value".
//!
//! Worked example:
//!
//! ```text
//! decimal 0xA0 (= 160)
//!     = 0b 1 0100000     // 8 bits, doesn't fit in 7
//!     → split into two 7-bit groups, MSB-first:
//!         high = 0b 0000001
//!         low  = 0b 0100000
//!     → set continuation bit on every byte except the last:
//!         0x81 0x20
//! ```
//!
//! On the encode side we always write the minimum number of bytes
//! required (no leading 0x80 padding), which is exactly what the spec
//! mandates: WAP-237 §4.3.1 states *"The unsigned integer MUST be
//! encoded in the smallest encoding possible. In other words, the
//! encoded value MUST NOT start with an octet with the value 0x80."*
//!
//! On the decode side we offer two readers. [`read_mbi_u32`] is **lax**:
//! it accepts a bounded number of leading 0x80 padding octets (some
//! files in the wild pad despite the MUST NOT), as long as the running
//! value stays within the `u32` range. [`read_mbi_u32_strict`] enforces
//! the §4.3.1 MUST NOT verbatim — a leading 0x80 octet is rejected as
//! [`WbmpError::InvalidData`]. The strict reader feeds the strict header
//! / decode entry points so a fully-conformant parse can be requested.
//!
//! All MBIs WBMP actually uses (Type, Width, Height) are unsigned and
//! comfortably fit in `u32` — the spec caps the bitmap dimensions at
//! the device's display capability. We reject MBIs whose accumulated
//! value would overflow `u32` so callers don't get silent truncation.

use crate::error::{Result, WbmpError};

/// Maximum number of bytes a 32-bit MBI can occupy when minimally
/// encoded. A `u32` carries 32 payload bits; at 7 payload bits per
/// MBI byte the worst case is `ceil(32 / 7) = 5` bytes.
pub const MAX_U32_MBI_BYTES: usize = 5;

/// Hard ceiling on the number of bytes the MBI decoder will read for
/// a single value. `MAX_U32_MBI_BYTES` (5) plus a 2-byte allowance for
/// the redundant leading-0x80 padding some reference encoders produce.
/// Beyond this we error rather than chase a pathologically padded /
/// adversarial run of continuation bytes.
pub const MAX_MBI_BYTES: usize = MAX_U32_MBI_BYTES + 2;

/// Decode a single MBI starting at `bytes[*offset]`. On success the
/// offset is advanced past the consumed bytes and the decoded value is
/// returned.
///
/// Returns `WbmpError::InvalidData` if the encoding is truncated (the
/// last byte read still has its continuation bit set) or if the
/// accumulated value would exceed `u32::MAX`.
pub fn read_mbi_u32(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value: u64 = 0;
    let start = *offset;
    let mut bytes_read: usize = 0;

    loop {
        if *offset >= bytes.len() {
            return Err(WbmpError::invalid(format!(
                "MBI starting at byte {start}: truncated (continuation bit still set)"
            )));
        }
        let b = bytes[*offset];
        *offset += 1;
        bytes_read += 1;

        // Shift in the 7 payload bits.
        value = (value << 7) | (b & 0x7F) as u64;
        if value > u32::MAX as u64 {
            return Err(WbmpError::invalid(format!(
                "MBI starting at byte {start}: value exceeds u32::MAX"
            )));
        }

        // Cap MBI length at a hard ceiling even when the running
        // value still fits. Anything longer is either pathologically
        // padded or just plain malformed; bounding here keeps decode
        // O(1) in the face of a `[0x80; 1_000_000]`-style attack and
        // still leaves room for a couple of legitimate leading-0x80
        // padding bytes seen in some reference test vectors.
        if bytes_read > MAX_MBI_BYTES {
            return Err(WbmpError::invalid(format!(
                "MBI starting at byte {start}: more than {MAX_MBI_BYTES} bytes"
            )));
        }

        // High bit clear → final byte.
        if (b & 0x80) == 0 {
            return Ok(value as u32);
        }
    }
}

/// Decode a single MBI starting at `bytes[*offset]`, enforcing the
/// §4.3.1 shortest-encoding requirement.
///
/// WAP-237 §4.3.1 (page 9): *"The unsigned integer MUST be encoded in
/// the smallest encoding possible. In other words, the encoded value
/// MUST NOT start with an octet with the value 0x80."* This reader
/// rejects such a leading octet — a value of `0x80` as the first byte
/// of the sequence means a redundant continuation octet that carries no
/// payload bits and could have been omitted. Every other check (truncated
/// continuation, `u32::MAX` overflow, the [`MAX_MBI_BYTES`] ceiling) is
/// identical to [`read_mbi_u32`]; on success the offset is advanced past
/// the consumed bytes and the decoded value returned.
///
/// Note the spec wording forbids a *leading* `0x80` specifically — a
/// `0x80` octet later in the sequence is a legitimate "this 7-bit group
/// is all zero, more bytes follow" group (e.g. `0x4000` encodes as
/// `0x81 0x80 0x00`), so only the first octet of the sequence is checked.
pub fn read_mbi_u32_strict(bytes: &[u8], offset: &mut usize) -> Result<u32> {
    let start = *offset;
    if start < bytes.len() && bytes[start] == 0x80 {
        return Err(WbmpError::invalid(format!(
            "MBI starting at byte {start}: leading octet 0x80 violates the \
             shortest-encoding requirement (§4.3.1)"
        )));
    }
    read_mbi_u32(bytes, offset)
}

/// Append `value` to `out` as an MBI.
///
/// Always emits the minimum number of bytes (no leading 0x80 padding):
/// 1 byte for `0..=0x7F`, 2 bytes for `0x80..=0x3FFF`, and so on, up
/// to a maximum of 5 bytes for the full `u32` range.
pub fn write_mbi_u32(value: u32, out: &mut Vec<u8>) {
    // Emit 7-bit groups MSB-first, with the continuation bit set on
    // every byte except the last.
    //
    // Strategy: collect the groups LSB-first into a small stack array
    // (max 5 entries for a u32), then emit them in reverse.
    let mut groups = [0u8; MAX_U32_MBI_BYTES];
    let mut count = 0usize;
    let mut v = value;
    loop {
        groups[count] = (v as u8) & 0x7F;
        count += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }

    // count >= 1 here (we always push at least once, even for value=0).
    for i in (0..count).rev() {
        let mut byte = groups[i];
        if i != 0 {
            byte |= 0x80; // continuation: more bytes after this one
        }
        out.push(byte);
    }
}

/// Number of bytes [`write_mbi_u32`] would emit for `value`. Useful
/// for size estimation without actually encoding.
pub fn mbi_u32_len(value: u32) -> usize {
    let mut v = value;
    let mut n = 1usize;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: u32, expected: &[u8]) {
        let mut out = Vec::new();
        write_mbi_u32(value, &mut out);
        assert_eq!(out, expected, "encode {value:#x}");
        assert_eq!(mbi_u32_len(value), expected.len(), "len for {value:#x}");

        let mut offset = 0usize;
        let decoded = read_mbi_u32(expected, &mut offset).unwrap();
        assert_eq!(decoded, value, "decode {value:#x}");
        assert_eq!(offset, expected.len(), "consumed all bytes for {value:#x}");
    }

    #[test]
    fn worked_example_from_module_doc() {
        // 0xA0 = 160 → two MBI bytes: 0x81 0x20.
        roundtrip(0xA0, &[0x81, 0x20]);
    }

    #[test]
    fn single_byte_boundary() {
        // 0..=0x7F encodes in 1 byte.
        roundtrip(0, &[0x00]);
        roundtrip(0x7F, &[0x7F]);
    }

    #[test]
    fn two_byte_boundary() {
        // 0x80 → 0x81 0x00 is the smallest 2-byte MBI.
        roundtrip(0x80, &[0x81, 0x00]);
        // 0x3FFF is the largest 2-byte MBI.
        roundtrip(0x3FFF, &[0xFF, 0x7F]);
    }

    #[test]
    fn three_and_four_byte_boundaries() {
        // 0x4000 → 3-byte: 0x81 0x80 0x00.
        roundtrip(0x4000, &[0x81, 0x80, 0x00]);
        // Largest 3-byte.
        roundtrip(0x1F_FFFF, &[0xFF, 0xFF, 0x7F]);
        // Smallest 4-byte.
        roundtrip(0x20_0000, &[0x81, 0x80, 0x80, 0x00]);
    }

    #[test]
    fn five_byte_max_u32() {
        // u32::MAX uses the full 5 MBI bytes.
        roundtrip(u32::MAX, &[0x8F, 0xFF, 0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn truncated_mbi_is_invalid() {
        // Continuation bit set on the only available byte → no follow-up.
        let bytes = [0x81u8];
        let mut offset = 0;
        let err = read_mbi_u32(&bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn redundant_padding_is_accepted() {
        // 0x80 0x80 0x00 = value 0 with two leading padding bytes.
        // Spec doesn't outlaw redundant encodings; accept and produce 0.
        let bytes = [0x80u8, 0x80, 0x00];
        let mut offset = 0;
        let v = read_mbi_u32(&bytes, &mut offset).unwrap();
        assert_eq!(v, 0);
        assert_eq!(offset, 3);
    }

    #[test]
    fn overflow_exceeds_u32() {
        // Value > u32::MAX must error before silently truncating.
        let bytes = [0xFFu8, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
        let mut offset = 0;
        let err = read_mbi_u32(&bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn long_continuation_run_caps_at_max_mbi_bytes() {
        // 32 continuation bytes followed by a terminator — well past
        // MAX_MBI_BYTES. Reader must error before exhausting the run.
        let mut bytes = vec![0x80u8; 32];
        bytes.push(0x01);
        let mut offset = 0;
        let err = read_mbi_u32(&bytes, &mut offset).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
        // It bailed shortly past MAX_MBI_BYTES (the check fires after
        // incrementing the byte counter, so offset == MAX_MBI_BYTES+1).
        assert!(offset <= MAX_MBI_BYTES + 1);
    }

    #[test]
    fn two_byte_padding_at_cap_is_accepted() {
        // MAX_MBI_BYTES = 7, so two leading 0x80 padding bytes plus a
        // 5-byte u32 should still parse.
        let bytes = [0x80u8, 0x80, 0x8F, 0xFF, 0xFF, 0xFF, 0x7F];
        let mut offset = 0;
        let v = read_mbi_u32(&bytes, &mut offset).unwrap();
        assert_eq!(v, u32::MAX);
        assert_eq!(offset, MAX_MBI_BYTES);
    }

    #[test]
    fn strict_rejects_leading_0x80() {
        // §4.3.1 MUST NOT: the encoded value must not start with an
        // octet of value 0x80. The lax reader accepts `0x80 0x80 0x00`
        // (= value 0 with redundant padding); the strict reader rejects
        // it on the first octet.
        let bytes = [0x80u8, 0x80, 0x00];
        // Lax: still accepted (see redundant_padding_is_accepted).
        let mut off = 0;
        assert_eq!(read_mbi_u32(&bytes, &mut off).unwrap(), 0);
        // Strict: rejected, offset untouched (the check fires before any
        // byte is consumed).
        let mut off = 0;
        let err = read_mbi_u32_strict(&bytes, &mut off).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
        assert_eq!(off, 0);
        if let WbmpError::InvalidData(msg) = &err {
            assert!(
                msg.contains("0x80") && msg.contains("shortest"),
                "message should name the offending octet + rule: {msg}"
            );
        }
    }

    #[test]
    fn strict_accepts_shortest_encodings() {
        // Every minimally-encoded value must parse identically through
        // the strict reader. Includes the 0x4000 case whose *internal*
        // octet is 0x80 (0x81 0x80 0x00) — only a *leading* 0x80 is
        // forbidden, so this must be accepted.
        for &(value, enc) in &[
            (0u32, &[0x00u8][..]),
            (0x7F, &[0x7F][..]),
            (0x80, &[0x81, 0x00][..]),
            (0x4000, &[0x81, 0x80, 0x00][..]),
            (u32::MAX, &[0x8F, 0xFF, 0xFF, 0xFF, 0x7F][..]),
        ] {
            let mut off = 0;
            let v = read_mbi_u32_strict(enc, &mut off).unwrap();
            assert_eq!(v, value, "strict decode {value:#x}");
            assert_eq!(off, enc.len(), "consumed all bytes for {value:#x}");
        }
    }

    #[test]
    fn strict_offset_starts_at_nonzero() {
        // The leading-octet check is keyed off the *current* offset, not
        // byte 0 of the slice. A 0x80 sitting at the start of the slice
        // but past the offset (i.e. it's an interior continuation octet
        // of an earlier value) must not trip the strict reader.
        // Buffer: [0x4000 as 0x81 0x80 0x00][0x40 as 0x40].
        let bytes = [0x81u8, 0x80, 0x00, 0x40];
        let mut off = 0;
        assert_eq!(read_mbi_u32_strict(&bytes, &mut off).unwrap(), 0x4000);
        assert_eq!(off, 3);
        // Second value begins at offset 3 (0x40, no leading 0x80).
        assert_eq!(read_mbi_u32_strict(&bytes, &mut off).unwrap(), 0x40);
        assert_eq!(off, 4);
    }

    #[test]
    fn strict_still_rejects_truncated_and_overflow() {
        // The strict reader inherits the lax reader's truncation +
        // overflow guards (it only adds the leading-0x80 check).
        let mut off = 0;
        assert!(read_mbi_u32_strict(&[0x81u8], &mut off).is_err());
        let mut off = 0;
        assert!(read_mbi_u32_strict(&[0xFFu8, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F], &mut off).is_err());
    }

    #[test]
    fn writer_invariants_hold_across_value_sweep() {
        // Deterministic mirror of the `mbi` fuzz target's writer half: for
        // a spread of values covering every octet-count boundary plus a
        // dense low-range sweep, the encoding must (a) never begin with a
        // leading 0x80 (§4.3.1), (b) set the continuation bit on every
        // non-final octet and clear it on the final one, (c) have a length
        // equal to `mbi_u32_len` in 1..=5, and (d) decode back exactly
        // through both readers, consuming all emitted bytes.
        let mut values: Vec<u32> = (0u32..=0x200).collect();
        values.extend([
            0x7F,
            0x80,
            0x3FFF,
            0x4000,
            0x1F_FFFF,
            0x20_0000,
            0x0FFF_FFFF,
            0x1000_0000,
            0x7FFF_FFFF,
            0x8000_0000,
            u32::MAX - 1,
            u32::MAX,
        ]);
        for v in values {
            let mut buf = Vec::new();
            write_mbi_u32(v, &mut buf);
            assert_eq!(buf.len(), mbi_u32_len(v), "len estimator for {v:#x}");
            assert!(
                (1..=MAX_U32_MBI_BYTES).contains(&buf.len()),
                "len bound {v:#x}"
            );
            assert_ne!(buf[0], 0x80, "leading 0x80 for {v:#x}");
            let last = buf.len() - 1;
            for (i, &b) in buf.iter().enumerate() {
                assert_eq!((b & 0x80) != 0, i != last, "cont flag octet {i} of {v:#x}");
            }
            let mut off = 0;
            assert_eq!(
                read_mbi_u32(&buf, &mut off).unwrap(),
                v,
                "lax decode {v:#x}"
            );
            assert_eq!(off, buf.len(), "lax consumed all for {v:#x}");
            let mut soff = 0;
            assert_eq!(
                read_mbi_u32_strict(&buf, &mut soff).unwrap(),
                v,
                "strict {v:#x}"
            );
            assert_eq!(soff, buf.len(), "strict consumed all for {v:#x}");
        }
    }

    #[test]
    fn offset_advances_past_consumed_bytes_only() {
        // Pack two MBIs back-to-back; second decode must start where
        // the first ended.
        let mut buf = Vec::new();
        write_mbi_u32(0x1234, &mut buf);
        write_mbi_u32(0x40, &mut buf);
        let mut offset = 0;
        let a = read_mbi_u32(&buf, &mut offset).unwrap();
        let b = read_mbi_u32(&buf, &mut offset).unwrap();
        assert_eq!(a, 0x1234);
        assert_eq!(b, 0x40);
        assert_eq!(offset, buf.len());
    }
}
