//! The root vocabulary of the image-crate API contract
//! (`IMAGE_CRATE_API`): `probe` / `info` / `decode*` / `encode*`.
//!
//! Every function here is framework-free (builds with
//! `default-features = false`) and is the single implementation the
//! registry `Decoder` / `Encoder` adapters call.

use std::io::{Read, Write};

use crate::decoder;
use crate::encoder;
use crate::error::Result;
use crate::ext::MAX_EXT_FIELD_BYTES;
use crate::image::{Frame, ImageInfo, PlaneLayout, RgbImage, RgbaImage, WbmpImage};
use crate::mbi::peek_mbi_u32;
use crate::options::{DecodeOptions, EncodeOptions};

/// Largest width / height [`probe`] accepts as plausible. WBMP has no
/// magic number — a Type-0 file starts with a `0x00` octet — so the
/// sniff reads the whole header and treats an absurd dimension as
/// noise rather than a bitmap. `decode` itself is bounded by
/// [`DecodeOptions`], not by this.
pub const PROBE_MAX_DIMENSION: u32 = 16_384;

/// `true` when `bytes` begins with a plausible WBMP Type-0 header:
/// `Type` MBI `0`, a `FixHeaderField` octet (its extension-header
/// region, if flagged, well-formed and skipped), non-zero `Width` and
/// `Height` MBIs at most [`PROBE_MAX_DIMENSION`], and at least the
/// first pixel row present after the header. Total, allocation-free,
/// `false` on short input. Says nothing about the rest of the body —
/// see [`info`] / [`decode`].
///
/// A probe buffer may be a truncated preview, so only the first row
/// (not the whole image) is required.
pub fn probe(bytes: &[u8]) -> bool {
    sniff(bytes).is_some()
}

/// The allocation-free header walk behind [`probe`]: `(width, height,
/// data_offset)` for a plausible header.
pub(crate) fn sniff(bytes: &[u8]) -> Option<(u32, u32, usize)> {
    let mut off = 0usize;
    if peek_mbi_u32(bytes, &mut off)? != 0 {
        return None;
    }
    let fix = *bytes.get(off)?;
    off += 1;
    if fix & 0x80 != 0 {
        // ExtFields follow (§4.4.1); skip the region per its type with
        // exactly the lenient parser's acceptance rules (so `probe` ⇒
        // `info` succeeds on the header).
        let start = off;
        match (fix >> 5) & 0b11 {
            0b00 => loop {
                if off - start >= MAX_EXT_FIELD_BYTES {
                    return None;
                }
                let b = *bytes.get(off)?;
                off += 1;
                if b & 0x80 == 0 {
                    break;
                }
            },
            0b01 | 0b10 => {
                bytes.get(off)?;
                off += 1;
            }
            _ => loop {
                if off - start >= MAX_EXT_FIELD_BYTES {
                    return None;
                }
                let header = *bytes.get(off)?;
                off += 1;
                let ident = ((header >> 4) & 0b111) as usize;
                let value = (header & 0b1111) as usize;
                if ident == 0 || value == 0 {
                    return None;
                }
                off = off.checked_add(ident + value)?;
                if off > bytes.len() {
                    return None;
                }
                if header & 0x80 == 0 {
                    break;
                }
            },
        }
    }
    let width = peek_mbi_u32(bytes, &mut off)?;
    let height = peek_mbi_u32(bytes, &mut off)?;
    if width == 0 || height == 0 || width > PROBE_MAX_DIMENSION || height > PROBE_MAX_DIMENSION {
        return None;
    }
    // At least the first pixel row must be present.
    let stride = WbmpImage::row_stride(width);
    if bytes.len() < off.checked_add(stride)? {
        return None;
    }
    Some((width, height, off))
}

/// Header only: dimensions, native [`PixelFormat`] (`MonoBlack`),
/// `frames` (the main image plus the animated sub-images that fit in
/// the buffer, `1..=16`), the (always absent) alpha / metadata flags,
/// WBMP's default colour, plus the raw `FixHeaderField`, any
/// extension-header region and the pixel-data offset. The general-form
/// header is parsed (extension headers honoured); no pixel is read and
/// no limit applies.
///
/// Errors: [`crate::WbmpError::Unsupported`] for a non-zero `Type`,
/// [`crate::WbmpError::InvalidData`] for a truncated / oversized MBI,
/// a malformed extension region or a zero dimension.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    let h = decoder::parse_header_for(bytes, false)?;
    let layout = PlaneLayout::new(h.width, h.height)
        .map_err(|msg| crate::WbmpError::invalid(msg.to_string()))?;
    let extra = decoder::animated_frame_count(bytes.len(), h.data_offset, layout.total_bytes);
    Ok(ImageInfo::new(h.width, h.height, h.data_offset)
        .with_frames(1 + extra as u32)
        .with_ext(h.fix_header, h.ext_fields))
}

/// Decode the main image into its native layout
/// ([`PixelFormat::MonoBlack`]: one packed plane, `ceil(width / 8)`
/// bytes per row, 1 = white) with [`DecodeOptions::default`] (lenient
/// header, packed plane capped at 1 GiB).
pub fn decode(bytes: &[u8]) -> Result<WbmpImage> {
    decode_with(bytes, &DecodeOptions::default())
}

/// [`decode`] with explicit limits, strictness and polarity. Every
/// limit is checked against the header before the pixel buffer is
/// allocated ([`crate::WbmpError::LimitExceeded`]).
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<WbmpImage> {
    decoder::decode_image(bytes, opts)
}

/// Decode the main image straight to tightly packed 8-bit RGB (`0, 0,
/// 0` / `255, 255, 255` per pixel), default options.
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    Ok(RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// Decode the main image straight to tightly packed 8-bit RGBA (alpha
/// `255`), default options.
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    let img = decode(bytes)?;
    Ok(RgbaImage::new(img.width, img.height, img.to_rgba8()))
}

/// Decode the main image **and** every animated sub-image that follows
/// it (WAP-237 §4.2 / §4.5.1: up to 15 same-dimension planes after the
/// main image, no per-frame header) with [`DecodeOptions::default`].
///
/// `frames[0]` is the main image (identical to [`decode`]); every
/// frame carries the same layout, `delay` is always `None` (WBMP
/// defines no timing) and [`Frame::index`] is the stream position. A
/// conformant single-image file yields exactly one frame. A trailing
/// run shorter than one frame is ignored, like any trailing bytes.
pub fn decode_all(bytes: &[u8]) -> Result<Vec<Frame>> {
    decode_all_with(bytes, &DecodeOptions::default())
}

/// [`decode_all`] with explicit options. `max_bytes` bounds each
/// frame's plane; the §4.5.1 frame cap bounds the total.
pub fn decode_all_with(bytes: &[u8], opts: &DecodeOptions) -> Result<Vec<Frame>> {
    decoder::decode_frames(bytes, opts)
}

/// Read `r` to end and [`decode`] it. WBMP has no length field for its
/// trailing animated sub-images, so the whole input is buffered. Read
/// failures surface as [`crate::WbmpError::Io`].
pub fn decode_from<R: Read>(mut r: R) -> Result<WbmpImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Encode `image` as a complete WBMP Type-0 file. Both polarities
/// encode exactly (a `MonoWhite` plane is inverted to the wire form, a
/// padded plane repacked, padding bits zeroed); `color` and `metadata`
/// cannot be carried and are ignored; [`EncodeOptions::quantize`] does
/// not apply to 1-bit input. There is no `WbmpImage` WBMP cannot
/// represent, so [`crate::WbmpError::Unsupported`] is reserved for
/// geometry that overflows `usize`.
pub fn encode(image: &WbmpImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    encoder::encode_image(image, opts)
}

/// Encode tightly packed 8-bit RGB (`3 × width × height` bytes) as a
/// WBMP file: each pixel's Rec. 601 luma (`(299 R + 587 G + 114 B +
/// 500) / 1000`) is reduced to 1 bit per [`EncodeOptions::quantize`]
/// (threshold 128 by default, or Floyd–Steinberg dithering) — WBMP
/// cannot carry continuous tone, so this conversion is documented
/// rather than refused. A short buffer is
/// [`crate::WbmpError::InvalidData`].
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode(
        &WbmpImage::from_rgb8_with(width, height, rgb, opts.quantize)?,
        opts,
    )
}

/// Encode tightly packed 8-bit RGBA (`4 × width × height` bytes) as a
/// WBMP file. Alpha is **dropped** (WBMP has no alpha mechanism; a
/// transparent pixel keeps its colour samples), then the pixels
/// quantise as in [`encode_rgb8`].
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode(
        &WbmpImage::from_rgba8_with(width, height, rgba, opts.quantize)?,
        opts,
    )
}

/// Encode `width × height` 8-bit grey samples (one byte per pixel, no
/// row padding) as a WBMP file, quantised per
/// [`EncodeOptions::quantize`]. WBMP extra: the natural input for a
/// 1-bit format.
pub fn encode_gray8(width: u32, height: u32, gray: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode(
        &WbmpImage::from_gray8(width, height, gray, opts.quantize)?,
        opts,
    )
}

/// [`encode`] into a writer. Write failures surface as
/// [`crate::WbmpError::Io`].
pub fn encode_to<W: Write>(image: &WbmpImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::encode_frames;
    use crate::ext::{ExtFields, Parameter};
    use crate::header::write_header;
    use crate::image::{ColorInfo, PixelFormat, Plane};
    use crate::options::Quantize;
    use crate::WbmpError;

    /// `w × h` wire-polarity plane with a deterministic pattern,
    /// padding bits zero.
    fn pattern_bits(w: u32, h: u32) -> Vec<u8> {
        let layout = PlaneLayout::new(w, h).unwrap();
        let mut bits = vec![0u8; layout.total_bytes];
        for y in 0..h as usize {
            for x in 0..w as usize {
                if (x * 7 + y * 3) % 5 < 2 {
                    bits[y * layout.stride + (x >> 3)] |= 0x80 >> (x & 7);
                }
            }
        }
        bits
    }

    fn file(w: u32, h: u32, bits: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        write_header(w, h, &mut out);
        out.extend_from_slice(bits);
        out
    }

    #[test]
    fn probe_is_total_and_header_only() {
        assert!(!probe(b""));
        assert!(!probe(&[0x00]));
        assert!(!probe(&[0x00, 0x00, 0x01]));
        assert!(!probe(&[0x00, 0x00, 0x01, 0x01])); // no first row
        assert!(probe(&[0x00, 0x00, 0x01, 0x01, 0x80]));
        assert!(!probe(&[0x01, 0x00, 0x01, 0x01, 0x80])); // Type 1
        assert!(!probe(&[0x00, 0x00, 0x00, 0x01, 0x80])); // zero width
        assert!(!probe(b"\x89PNG\r\n\x1a\n"));
        // Absurd dimensions are noise.
        assert!(!probe(&[0x00, 0x00, 0x82, 0x80, 0x00, 0x01, 0, 0, 0, 0]));
        // A full file always probes; a preview holding one row too.
        let f = file(13, 7, &pattern_bits(13, 7));
        assert!(probe(&f));
        assert!(probe(&f[..6]));
        assert!(!probe(&f[..5]));
        // Extension headers are skipped: Type-11 pair "a=b" then 1×1.
        let ext = [0x00, 0xE0, 0x11, b'a', b'b', 0x01, 0x01, 0x80];
        assert!(probe(&ext));
        assert!(!probe(&ext[..7]));
        // Bitfield00 chain.
        assert!(probe(&[0x00, 0x80, 0x81, 0x01, 0x01, 0x01, 0x80]));
        // Reserved01 single octet.
        assert!(probe(&[0x00, 0xA0, 0x55, 0x01, 0x01, 0x80]));
        // Type-11 header with a zero size is malformed.
        assert!(!probe(&[0x00, 0xE0, 0x10, 0x01, 0x01, 0x80]));
    }

    #[test]
    fn info_reads_header_only() {
        let f = file(13, 7, &pattern_bits(13, 7));
        let i = info(&f).unwrap();
        assert_eq!((i.width, i.height), (13, 7));
        assert_eq!(i.format, PixelFormat::MonoBlack);
        assert_eq!(i.frames, 1);
        assert!(!i.has_alpha && !i.has_icc && !i.has_exif && !i.has_xmp);
        assert_eq!(i.color, ColorInfo::wbmp_default());
        assert_eq!(i.fix_header, 0);
        assert_eq!(i.ext_fields, None);
        assert_eq!(i.data_offset, 4);
        // Header alone (no pixels) is enough.
        assert_eq!(info(&f[..4]).unwrap().frames, 1);
        // Two animated sub-images + a partial tail → 3 frames.
        let mut anim = f.clone();
        anim.extend_from_slice(&pattern_bits(13, 7));
        anim.extend_from_slice(&pattern_bits(13, 7));
        anim.extend_from_slice(&[0xFF; 3]);
        assert_eq!(info(&anim).unwrap().frames, 3);
        // Extension headers surface.
        let ext = [0x00, 0xE0, 0x11, b'a', b'b', 0x01, 0x01, 0x80];
        let i = info(&ext).unwrap();
        assert_eq!(i.fix_header, 0xE0);
        assert_eq!(
            i.ext_fields,
            Some(ExtFields::ParameterPairs11(vec![Parameter::new(
                b"a".to_vec(),
                b"b".to_vec()
            )
            .unwrap()]))
        );
        assert_eq!(i.data_offset, 7);
        assert!(matches!(
            info(&[0x01, 0x00]),
            Err(WbmpError::Unsupported(_))
        ));
        assert!(matches!(info(&[0x00]), Err(WbmpError::InvalidData(_))));
    }

    #[test]
    fn decode_fills_native_layout_and_colour() {
        let bits = pattern_bits(13, 7);
        let img = decode(&file(13, 7, &bits)).unwrap();
        assert_eq!((img.width(), img.height()), (13, 7));
        assert_eq!(img.format(), PixelFormat::MonoBlack);
        assert_eq!(img.planes.len(), 1);
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.as_bytes().unwrap(), &bits[..]);
        assert_eq!(img.color, ColorInfo::wbmp_default());
        assert!(img.metadata.is_empty());
        assert!(img.is_tightly_packed());
    }

    #[test]
    fn decode_with_format_flips_polarity() {
        let bits = pattern_bits(13, 7);
        let f = file(13, 7, &bits);
        let inv = decode_with(
            &f,
            &DecodeOptions::default().with_format(PixelFormat::MonoWhite),
        )
        .unwrap();
        assert_eq!(inv.format(), PixelFormat::MonoWhite);
        let expect: Vec<u8> = bits
            .chunks_exact(2)
            .flat_map(|r| [!r[0], !r[1] & 0xF8])
            .collect();
        assert_eq!(inv.as_bytes().unwrap(), &expect[..]);
        assert_eq!(inv.to_rgb8(), decode(&f).unwrap().to_rgb8());
        // Encodes back to the same bytes.
        assert_eq!(encode(&inv, &EncodeOptions::default()).unwrap(), f);
    }

    #[test]
    fn rgb8_and_rgba8_raw_paths() {
        // 3×1: white, black, white.
        let f = file(3, 1, &[0b1010_0000]);
        let rgb = decode_rgb8(&f).unwrap();
        assert_eq!((rgb.width, rgb.height), (3, 1));
        assert_eq!(rgb.as_bytes(), &[255, 255, 255, 0, 0, 0, 255, 255, 255]);
        let rgba = decode_rgba8(&f).unwrap();
        assert_eq!(
            rgba.into_raw(),
            vec![255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
    }

    #[test]
    fn decode_all_returns_every_frame_in_order() {
        let a = pattern_bits(9, 3);
        let b: Vec<u8> = a.iter().map(|x| !x & 0x80).collect();
        let c = vec![0x80; 6];
        let mut f = file(9, 3, &a);
        f.extend_from_slice(&b);
        f.extend_from_slice(&c);
        f.push(0x42); // partial tail
        let frames = decode_all(&f).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].image, decode(&f).unwrap());
        assert_eq!(frames[0].index, 0);
        assert_eq!(frames[1].image.as_bytes().unwrap(), &b[..]);
        assert_eq!(frames[1].index, 1);
        assert_eq!(frames[2].image.as_bytes().unwrap(), &c[..]);
        assert!(frames.iter().all(|fr| fr.delay.is_none()));
        assert_eq!(info(&f).unwrap().frames, 3);
        // Single image → one frame; a polarity request applies to all.
        assert_eq!(decode_all(&file(9, 3, &a)).unwrap().len(), 1);
        let inv = decode_all_with(
            &f,
            &DecodeOptions::default().with_format(PixelFormat::MonoWhite),
        )
        .unwrap();
        assert!(inv
            .iter()
            .all(|fr| fr.image.format() == PixelFormat::MonoWhite));
        // Cap at 16 frames.
        let mut big = file(8, 1, &[0x00]);
        big.extend_from_slice(&[0x01; 40]);
        assert_eq!(decode_all(&big).unwrap().len(), 16);
        assert_eq!(info(&big).unwrap().frames, 16);
    }

    #[test]
    fn encode_frames_is_the_inverse_of_decode_all() {
        let imgs: Vec<WbmpImage> = (0..4)
            .map(|i| {
                let mut bits = pattern_bits(11, 5);
                bits[0] ^= i;
                WbmpImage::from_bits(11, 5, bits).unwrap()
            })
            .collect();
        let bytes = encode_frames(&imgs, &EncodeOptions::default()).unwrap();
        let back = decode_all(&bytes).unwrap();
        assert_eq!(back.len(), 4);
        for (fr, img) in back.iter().zip(&imgs) {
            assert_eq!(&fr.image, img);
        }
        assert_eq!(
            encode_frames(&imgs[..1], &EncodeOptions::default()).unwrap(),
            encode(&imgs[0], &EncodeOptions::default()).unwrap()
        );
        // A MonoWhite frame is brought to the wire polarity.
        let mut mixed = imgs.clone();
        mixed[2] = mixed[2].clone().into_format(PixelFormat::MonoWhite);
        assert_eq!(
            encode_frames(&mixed, &EncodeOptions::default()).unwrap(),
            bytes
        );
        // Errors.
        assert!(matches!(
            encode_frames(&[], &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
        let mut odd = imgs.clone();
        odd.push(WbmpImage::from_bits(8, 1, vec![0]).unwrap());
        assert!(matches!(
            encode_frames(&odd, &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
        let many: Vec<WbmpImage> = (0..17).map(|_| imgs[0].clone()).collect();
        assert!(matches!(
            encode_frames(&many, &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
    }

    #[test]
    fn lossless_round_trip_pinned() {
        for (w, h) in [(1u32, 1u32), (8, 8), (13, 7), (159, 33), (64, 1), (1, 64)] {
            let img = WbmpImage::from_bits(w, h, pattern_bits(w, h)).unwrap();
            let bytes = encode(&img, &EncodeOptions::default()).unwrap();
            assert_eq!(decode(&bytes).unwrap(), img, "{w}×{h}");
            assert_eq!(
                decode_with(&bytes, &DecodeOptions::default().with_strict(true)).unwrap(),
                img
            );
            assert_eq!(
                bytes.len(),
                info(&bytes).unwrap().data_offset + img.data().len()
            );
        }
    }

    #[test]
    fn encode_repacks_padded_and_dirty_planes() {
        // 11×2 with a 3-byte stride and dirty padding bits.
        let img = WbmpImage::packed(
            11,
            2,
            PixelFormat::MonoBlack,
            3,
            vec![0xFF, 0xFF, 0x99, 0x0F, 0x1F, 0x99],
        )
        .unwrap();
        let bytes = encode(&img, &EncodeOptions::default()).unwrap();
        assert_eq!(&bytes[4..], &[0xFF, 0xE0, 0x0F, 0x00]);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.to_rgb8(), img.to_rgb8());
    }

    #[test]
    fn encode_with_ext_fields_decodes_leniently_only() {
        let img = WbmpImage::from_bits(8, 1, vec![0x5A]).unwrap();
        let ext =
            ExtFields::ParameterPairs11(vec![
                Parameter::new(b"fmt".to_vec(), b"wbmp0".to_vec()).unwrap()
            ]);
        let opts = EncodeOptions::default()
            .with_ext_fields(ext.clone())
            .with_strict(true);
        let bytes = encode(&img, &opts).unwrap();
        assert_eq!(bytes[1], 0xE0);
        assert_eq!(decode(&bytes).unwrap(), img);
        assert_eq!(info(&bytes).unwrap().ext_fields, Some(ext));
        assert!(matches!(
            decode_with(&bytes, &DecodeOptions::default().with_strict(true)),
            Err(WbmpError::InvalidData(_))
        ));
        // Strict writer rejects an out-of-class value.
        let bad = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"k".to_vec(),
            value: b"a-b".to_vec(),
        }]);
        assert!(matches!(
            encode(
                &img,
                &EncodeOptions::default()
                    .with_ext_fields(bad)
                    .with_strict(true)
            ),
            Err(WbmpError::InvalidData(_))
        ));
    }

    #[test]
    fn strict_rejects_what_lenient_tolerates() {
        // Non-zero FixHeaderField with the presence bit clear: lenient
        // treats it as opaque (no ExtFields), strict rejects it.
        let f = [0x00, 0x1F, 0x08, 0x01, 0xAA];
        assert_eq!(decode(&f).unwrap().as_bytes().unwrap(), &[0xAA]);
        assert!(matches!(
            decode_with(&f, &DecodeOptions::default().with_strict(true)),
            Err(WbmpError::InvalidData(_))
        ));
        // Redundantly padded width MBI.
        let f = [0x00, 0x00, 0x80, 0x08, 0x01, 0xAA];
        assert!(decode(&f).is_ok());
        assert!(matches!(
            decode_with(&f, &DecodeOptions::default().with_strict(true)),
            Err(WbmpError::InvalidData(_))
        ));
        // Both reject a non-zero Type and a truncated body.
        assert!(matches!(
            decode(&[0x01, 0x00, 0x08, 0x01, 0xAA]),
            Err(WbmpError::Unsupported(_))
        ));
        assert!(matches!(
            decode(&[0x00, 0x00, 0x10, 0x01, 0xAA]),
            Err(WbmpError::InvalidData(_))
        ));
    }

    #[test]
    fn raw_encode_paths_quantise_as_documented() {
        // 9×1 grey ramp, threshold 128 → bits 0000_0111 1.
        let gray: Vec<u8> = [0, 32, 64, 96, 127, 128, 160, 200, 255].to_vec();
        let t = encode_gray8(9, 1, &gray, &EncodeOptions::default()).unwrap();
        assert_eq!(&t[4..], &[0b0000_0111, 0b1000_0000]);
        let t64 = encode_gray8(9, 1, &gray, &EncodeOptions::default().with_threshold(64)).unwrap();
        assert_eq!(&t64[4..], &[0b0011_1111, 0b1000_0000]);
        // Dither on saturated input agrees with the threshold.
        let sat: Vec<u8> = gray
            .iter()
            .map(|&g| if g >= 128 { 255 } else { 0 })
            .collect();
        assert_eq!(
            encode_gray8(9, 1, &sat, &EncodeOptions::default().with_dither()).unwrap(),
            t
        );
        // RGB / RGBA → luma; alpha ignored.
        let rgb: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g]).collect();
        assert_eq!(
            encode_rgb8(9, 1, &rgb, &EncodeOptions::default()).unwrap(),
            t
        );
        let rgba: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g, 0]).collect();
        assert_eq!(
            encode_rgba8(9, 1, &rgba, &EncodeOptions::default()).unwrap(),
            t
        );
        // Decoding the result expands to 0 / 255.
        assert_eq!(
            decode_rgb8(&t).unwrap().data,
            sat.iter().flat_map(|&g| [g, g, g]).collect::<Vec<u8>>()
        );
        // Short buffers are rejected without panicking.
        assert!(matches!(
            encode_rgb8(2, 2, &[0; 11], &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            encode_rgba8(0, 2, &[], &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
        assert!(matches!(
            encode_gray8(3, 3, &[0; 8], &EncodeOptions::default()),
            Err(WbmpError::InvalidData(_))
        ));
        assert_eq!(
            EncodeOptions::default()
                .with_quantize(Quantize::Dither)
                .quantize,
            Quantize::Dither
        );
    }

    #[test]
    fn decode_with_limits_fire_before_allocation() {
        let f = file(64, 64, &pattern_bits(64, 64));
        let o = DecodeOptions::default().with_max_width(63u32);
        assert!(matches!(
            decode_with(&f, &o),
            Err(WbmpError::LimitExceeded(_))
        ));
        let o = DecodeOptions::default().with_max_height(63u32);
        assert!(matches!(
            decode_with(&f, &o),
            Err(WbmpError::LimitExceeded(_))
        ));
        let o = DecodeOptions::default().with_max_pixels(4095u64);
        assert!(matches!(
            decode_with(&f, &o),
            Err(WbmpError::LimitExceeded(_))
        ));
        let o = DecodeOptions::default().with_max_bytes(511u64);
        assert!(matches!(
            decode_with(&f, &o),
            Err(WbmpError::LimitExceeded(_))
        ));
        let o = DecodeOptions::default()
            .with_max_pixels(4096u64)
            .with_max_bytes(512u64);
        assert!(decode_with(&f, &o).is_ok());
        // A hostile header claiming 2³²−1 × 2³²−1 trips the default
        // 1 GiB cap without touching the allocator (no body present).
        let hostile = [
            0x00, 0x00, 0x8F, 0xFF, 0xFF, 0xFF, 0x7F, 0x8F, 0xFF, 0xFF, 0xFF, 0x7F,
        ];
        assert!(matches!(decode(&hostile), Err(WbmpError::LimitExceeded(_))));
        assert!(matches!(
            decode_all(&hostile),
            Err(WbmpError::LimitExceeded(_))
        ));
        // Lifting the cap falls through to the truncation check.
        assert!(matches!(
            decode_with(&hostile, &DecodeOptions::default().unlimited()),
            Err(WbmpError::InvalidData(_))
        ));
        // `info` is limit-free.
        assert_eq!(info(&hostile).unwrap().width, u32::MAX);
    }

    #[test]
    fn decode_from_and_encode_to_round_trip() {
        let img = WbmpImage::from_bits(13, 7, pattern_bits(13, 7)).unwrap();
        let mut out = Vec::new();
        encode_to(&img, &EncodeOptions::default(), &mut out).unwrap();
        let back = decode_from(std::io::Cursor::new(&out)).unwrap();
        assert_eq!(back, img);

        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        assert!(matches!(decode_from(Failing), Err(WbmpError::Io(_))));
        struct Sink;
        impl Write for Sink {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            encode_to(&img, &EncodeOptions::default(), Sink),
            Err(WbmpError::Io(_))
        ));
        let e: WbmpError = std::io::Error::other("x").into();
        assert!(std::error::Error::source(&e).is_some());
        assert!(e.to_string().starts_with("io: "));
    }

    #[test]
    fn hostile_short_inputs_never_panic() {
        for a in 0u8..=255 {
            for b in [0u8, 0x80, 0xE0, 0xFF, 0x01] {
                for tail in [
                    &[][..],
                    &[0x01][..],
                    &[0x01, 0x01][..],
                    &[0x01, 0x01, 0x80][..],
                ] {
                    let mut buf = vec![a, b];
                    buf.extend_from_slice(tail);
                    let _ = probe(&buf);
                    let _ = info(&buf);
                    let _ = decode(&buf);
                    let _ = decode_all(&buf);
                    let _ = decode_with(&buf, &DecodeOptions::default().with_strict(true));
                }
            }
        }
    }

    #[test]
    #[allow(deprecated)]
    fn deprecated_wrappers_agree_with_the_contract_path() {
        let bits = pattern_bits(13, 7);
        let old = crate::encode_wbmp(13, 7, &bits).unwrap();
        let img = WbmpImage::from_bits(13, 7, bits.clone()).unwrap();
        assert_eq!(old, encode(&img, &EncodeOptions::default()).unwrap());
        assert_eq!(crate::parse_wbmp(&old).unwrap(), decode(&old).unwrap());
        assert_eq!(
            crate::parse_wbmp_strict(&old).unwrap(),
            decode_with(&old, &DecodeOptions::default().with_strict(true)).unwrap()
        );
        assert_eq!(
            crate::parse_wbmp_as(&old, PixelFormat::MonoWhite).unwrap(),
            decode_with(
                &old,
                &DecodeOptions::default().with_format(PixelFormat::MonoWhite)
            )
            .unwrap()
        );
        let gray: Vec<u8> = (0..91).map(|i| (i * 3) as u8).collect();
        assert_eq!(
            crate::encode_wbmp_from_threshold(13, 7, &gray, 100).unwrap(),
            encode_gray8(13, 7, &gray, &EncodeOptions::default().with_threshold(100)).unwrap()
        );
        assert_eq!(
            crate::encode_wbmp_from_dither(13, 7, &gray).unwrap(),
            encode_gray8(13, 7, &gray, &EncodeOptions::default().with_dither()).unwrap()
        );
        let anim = crate::parse_wbmp_frames(&old).unwrap();
        assert_eq!(anim.frames.len(), 1);
        assert_eq!(anim.main_image(), img);
        assert_eq!(
            crate::encode_wbmp_frames(13, 7, &[&bits, &bits]).unwrap(),
            encode_frames(&[img.clone(), img.clone()], &EncodeOptions::default()).unwrap()
        );
        let h = crate::parse_header(&old).unwrap();
        let i = info(&old).unwrap();
        assert_eq!(
            (h.width, h.height, h.data_offset),
            (i.width, i.height, i.data_offset)
        );
        let lim: DecodeOptions = crate::WbmpLimits::unbounded().into();
        assert_eq!(lim.max_width, Some(u32::MAX));
        let _ = Plane::new(2, vec![0; 14]);
    }
}
