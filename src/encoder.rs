//! WBMP Type-0 encoder.
//!
//! The contract entry points live at the crate root ([`crate::encode`],
//! [`crate::encode_rgb8`], [`crate::encode_rgba8`],
//! [`crate::encode_gray8`], [`crate::encode_to`]) and call the
//! crate-internal `encode_image` here; [`encode_frames`] is the
//! animated-stream depth API (WAP-237 §4.2 / §4.5.1). The pre-contract
//! `encode_wbmp*` family remains as `#[deprecated]` wrappers for one
//! release. With the default `registry` feature on, the gated
//! `WbmpEncoder` trait impl wraps the same functions for the
//! `oxideav_core::Encoder` surface.
//!
//! WBMP Type 0 is uncompressed: every writer here is a header prefix
//! (`Type`, `FixHeaderField`, optional `ExtFields`, `Width`, `Height`)
//! followed by the packed planes verbatim, so
//! `decode(encode(img)) == img` holds bit-exactly.

use crate::decoder::MAX_ANIMATED_IMAGES;
use crate::error::{Result, WbmpError};
use crate::ext::ExtFields;
use crate::header::{write_header, write_header_ext};
use crate::image::{PlaneLayout, WbmpImage};
use crate::options::{EncodeOptions, Quantize};

#[cfg(feature = "registry")]
use oxideav_core::Encoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, TimeBase};

// ---------------------------------------------------------------------------
// Crate-internal implementation.
// ---------------------------------------------------------------------------

/// Serialise a complete file: header per `opts` (conformant Type-0, or
/// the general form when `opts.ext_fields` is set) followed by every
/// plane in `planes`, each exactly `ceil(width / 8) × height` bytes of
/// wire-polarity bits. `planes` is non-empty and at most `1 +
/// MAX_ANIMATED_IMAGES` long; dimensions are non-zero.
fn write_file<'a>(
    width: u32,
    height: u32,
    planes: impl ExactSizeIterator<Item = &'a [u8]>,
    opts: &EncodeOptions,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "WBMP: zero dimension (width={width}, height={height})"
        )));
    }
    let count = planes.len();
    if count == 0 {
        return Err(WbmpError::invalid(
            "WBMP: at least the main image frame is required",
        ));
    }
    // §4.5.1: at most 15 animated sub-images follow the main image.
    if count > 1 + MAX_ANIMATED_IMAGES {
        return Err(WbmpError::invalid(format!(
            "WBMP: {count} frames exceeds the §4.5.1 maximum of {} \
             (main image + {MAX_ANIMATED_IMAGES} animated sub-images)",
            1 + MAX_ANIMATED_IMAGES,
        )));
    }
    let layout = PlaneLayout::new(width, height)
        .map_err(|msg| WbmpError::unsupported(format!("WBMP: {msg}")))?;

    // Header is at most 1 + 1 + 5 + 5 = 12 bytes without extension
    // headers; the ExtFields region is bounded by MAX_EXT_FIELD_BYTES.
    let body_bytes = layout.total_bytes.saturating_mul(count);
    let mut out = Vec::with_capacity(12 + body_bytes);
    match &opts.ext_fields {
        None => write_header(width, height, &mut out),
        Some(ext) => write_header_ext(width, height, Some(ext), opts.strict, &mut out)?,
    }
    for (i, plane) in planes.enumerate() {
        if plane.len() != layout.total_bytes {
            return Err(WbmpError::invalid(format!(
                "WBMP: frame {i} plane length {} != stride*height {}",
                plane.len(),
                layout.total_bytes,
            )));
        }
        out.extend_from_slice(plane);
    }
    Ok(out)
}

/// Encode one image: its plane is brought to the wire polarity /
/// packing ([`WbmpImage::wire_bits`]) and written after the header.
/// `color` and `metadata` cannot be carried and are ignored.
pub(crate) fn encode_image(image: &WbmpImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    // `WbmpImage::new` validated the geometry, so `wire_bits` is total.
    let bits = image.wire_bits();
    write_file(
        image.width,
        image.height,
        std::iter::once(bits.as_ref()),
        opts,
    )
}

/// Encode an animated WBMP stream: `frames[0]` is the main image,
/// `frames[1..]` (at most [`MAX_ANIMATED_IMAGES`]) the animated
/// sub-images in presentation order, all sharing the main image's
/// dimensions (WAP-237 §4.2: `Image-data = Main-image
/// 0*15Animated-image`, one header, no per-frame header or timing).
///
/// The exact inverse of [`crate::decode_all`]; a single-element
/// `frames` is byte-identical to [`crate::encode`] of that image.
/// Either polarity is accepted per frame (each is brought to the wire
/// layout). Errors with [`WbmpError::InvalidData`] for an empty slice,
/// more than 16 frames, or a frame whose dimensions differ from the
/// main image's.
pub fn encode_frames(frames: &[WbmpImage], opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode_images(frames.iter(), opts)
}

/// [`encode_frames`] over borrowed images; backs both it and
/// [`crate::encode_all`] (which borrows through `Frame::image`).
pub(crate) fn encode_images<'a>(
    frames: impl ExactSizeIterator<Item = &'a WbmpImage> + Clone,
    opts: &EncodeOptions,
) -> Result<Vec<u8>> {
    let Some(main) = frames.clone().next() else {
        return Err(WbmpError::invalid(
            "WBMP: at least the main image frame is required",
        ));
    };
    for (i, f) in frames.clone().enumerate() {
        if f.width != main.width || f.height != main.height {
            return Err(WbmpError::invalid(format!(
                "WBMP: frame {i} is {}×{}, the main image is {}×{} (all frames share the header dimensions)",
                f.width, f.height, main.width, main.height
            )));
        }
    }
    let bits: Vec<_> = frames.map(|f| f.wire_bits()).collect();
    write_file(
        main.width,
        main.height,
        bits.iter().map(|b| b.as_ref()),
        opts,
    )
}

// ---------------------------------------------------------------------------
// Registry-side Encoder trait surface.
// ---------------------------------------------------------------------------

/// Factory registered with the codec registry. `params.options` is
/// parsed as [`EncodeOptions`] (`quantize` / `threshold`, see the
/// `CodecOptionsStruct` impl in [`crate::registry`]).
#[cfg(feature = "registry")]
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let options: EncodeOptions = oxideav_core::parse_options(&params.options)?;
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    out_params.options = params.options.clone();
    Ok(Box::new(WbmpEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        options,
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct WbmpEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    options: EncodeOptions,
    pending: Option<Vec<u8>>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Encoder for WbmpEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "WBMP encoder: expected video frame",
                ))
            }
        };
        let format = self.out_params.pixel_format.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: pixel_format missing in CodecParameters")
        })?;
        let width = self.out_params.width.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: width missing in CodecParameters")
        })?;
        let height = self.out_params.height.ok_or_else(|| {
            oxideav_core::Error::invalid("WBMP encoder: height missing in CodecParameters")
        })?;

        let image = match format {
            // The 1-bit layouts go through the frame bridge (either
            // polarity; `encode_image` brings them to the wire form).
            PixelFormat::MonoBlack | PixelFormat::MonoWhite => {
                WbmpImage::from_video_frame(vf, &self.out_params)?
            }
            // Convenience: accept an 8-bit Gray plane and quantise it
            // per the encoder options (threshold 128 by default).
            PixelFormat::Gray8 => {
                let plane = vf
                    .image_planes()
                    .first()
                    .ok_or_else(|| oxideav_core::Error::invalid("WBMP encoder: no planes"))?;
                let gray = repack_gray8(plane, width, height)?;
                WbmpImage::from_gray8(width, height, &gray, self.options.quantize)?
            }
            other => {
                return Err(oxideav_core::Error::unsupported(format!(
                "WBMP encoder: unsupported pixel format {other:?} (MonoBlack / MonoWhite / Gray8)"
            )))
            }
        };
        self.pending = Some(encode_image(&image, &self.options)?);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

/// Tightly pack a `Gray8` frame plane (any stride ≥ width) into `width
/// × height` bytes.
#[cfg(feature = "registry")]
fn repack_gray8(
    plane: &oxideav_core::VideoPlane,
    width: u32,
    height: u32,
) -> oxideav_core::Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if plane.stride < w {
        return Err(oxideav_core::Error::invalid(format!(
            "WBMP encoder: Gray8 stride {} below width {w}",
            plane.stride
        )));
    }
    if plane.stride == w && plane.data.len() == w * h {
        return Ok(plane.data.clone());
    }
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = plane
            .data
            .get(y * plane.stride..y * plane.stride + w)
            .ok_or_else(|| oxideav_core::Error::invalid("WBMP encoder: Gray8 plane too short"))?;
        out.extend_from_slice(row);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Deprecated pre-contract entry points (one release).
// ---------------------------------------------------------------------------

/// Encode a WBMP Type-0 file from an already-packed monochrome bit
/// plane (`ceil(width / 8) × height` bytes, MSB-first, 1 = white).
#[deprecated(
    note = "use oxideav_wbmp::encode(&WbmpImage::from_bits(..)?, &opts) (IMAGE_CRATE_API)"
)]
pub fn encode_wbmp(width: u32, height: u32, mono_bits: &[u8]) -> Result<Vec<u8>> {
    write_file(
        width,
        height,
        std::iter::once(mono_bits),
        &EncodeOptions::default(),
    )
}

/// Encode a general-form (§4.4.1) WBMP file with an optional
/// [`ExtFields`] region — the pre-contract spelling of
/// [`crate::encode`] with [`EncodeOptions::ext_fields`].
#[deprecated(
    note = "use oxideav_wbmp::encode(.., &EncodeOptions::new().with_ext_fields(..)) (IMAGE_CRATE_API)"
)]
pub fn encode_wbmp_ext(
    width: u32,
    height: u32,
    mono_bits: &[u8],
    ext_fields: Option<&ExtFields>,
    strict: bool,
) -> Result<Vec<u8>> {
    let opts = EncodeOptions::default()
        .with_ext_fields(ext_fields.cloned())
        .with_strict(strict);
    write_file(width, height, std::iter::once(mono_bits), &opts)
}

/// Encode an animated WBMP Type-0 file from packed planes — the
/// pre-contract spelling of [`encode_frames`].
#[deprecated(note = "use oxideav_wbmp::encode_frames (IMAGE_CRATE_API)")]
pub fn encode_wbmp_frames(width: u32, height: u32, frames: &[&[u8]]) -> Result<Vec<u8>> {
    write_file(
        width,
        height,
        frames.iter().copied(),
        &EncodeOptions::default(),
    )
}

/// Threshold an 8-bit grayscale buffer into a 1-bit plane and wrap it
/// in a WBMP Type-0 file (`>= threshold` → white).
#[deprecated(
    note = "use oxideav_wbmp::encode_gray8(.., &EncodeOptions::new().with_threshold(t)) (IMAGE_CRATE_API)"
)]
pub fn encode_wbmp_from_threshold(
    width: u32,
    height: u32,
    gray: &[u8],
    threshold: u8,
) -> Result<Vec<u8>> {
    legacy_gray(width, height, gray, Quantize::Threshold(threshold))
}

/// Floyd–Steinberg-dither an 8-bit grayscale buffer into a WBMP Type-0
/// file.
#[deprecated(
    note = "use oxideav_wbmp::encode_gray8(.., &EncodeOptions::new().with_dither()) (IMAGE_CRATE_API)"
)]
pub fn encode_wbmp_from_dither(width: u32, height: u32, gray: &[u8]) -> Result<Vec<u8>> {
    legacy_gray(width, height, gray, Quantize::Dither)
}

/// The pre-contract grey helpers required `gray.len() == width ×
/// height` exactly (the contract path tolerates a longer buffer).
fn legacy_gray(width: u32, height: u32, gray: &[u8], rule: Quantize) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(WbmpError::invalid(format!(
            "WBMP: zero dimension (width={width}, height={height})"
        )));
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| WbmpError::invalid("WBMP: width × height overflows usize"))?;
    if gray.len() != pixel_count {
        return Err(WbmpError::invalid(format!(
            "WBMP: gray length {} != width*height {pixel_count}",
            gray.len()
        )));
    }
    let image = WbmpImage::from_gray8(width, height, gray, rule)?;
    encode_image(&image, &EncodeOptions::default())
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::decoder::{parse_wbmp, parse_wbmp_ext, parse_wbmp_frames};
    use crate::ext::{ExtFields, Parameter};

    #[test]
    fn ext_none_matches_encode_wbmp() {
        // encode_wbmp_ext with no ExtFields must be byte-identical to
        // encode_wbmp and round-trip through plain parse_wbmp.
        let bits = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];
        let plain = encode_wbmp(8, 8, &bits).unwrap();
        let ext = encode_wbmp_ext(8, 8, &bits, None, false).unwrap();
        assert_eq!(plain, ext, "no-ExtFields output equals encode_wbmp");
        let img = parse_wbmp(&ext).unwrap();
        assert_eq!(img.planes[0].data, bits);
    }

    #[test]
    fn ext_type11_roundtrips_through_parse_wbmp_ext() {
        // A non-conformant Type-0 stream carrying a Type-11 parameter pair
        // must round-trip through parse_wbmp_ext: both the image plane and
        // the ExtFields come back intact.
        let bits = [0xF0, 0x0F]; // a clean 16×1 plane (stride 2).
        let region = ExtFields::ParameterPairs11(vec![
            Parameter::new(b"k".to_vec(), b"V1").unwrap(),
            Parameter::new(b"name".to_vec(), b"abc123").unwrap(),
        ]);
        let encoded = encode_wbmp_ext(16, 1, &bits, Some(&region), false).unwrap();
        let decoded = parse_wbmp_ext(&encoded).unwrap();
        assert_eq!(decoded.image.width, 16);
        assert_eq!(decoded.image.height, 1);
        assert_eq!(decoded.image.planes[0].data, bits);
        assert_eq!(decoded.ext_fields, Some(region));
    }

    #[test]
    fn ext_strict_rejects_out_of_class_parameter() {
        // A Type-11 value byte outside ALPHA / DIGIT must be rejected by
        // the strict writer; the lax writer accepts it.
        let bits = [0x00, 0x00];
        let bad = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"k".to_vec(),
            value: b"a-b".to_vec(), // hyphen not ALPHA/DIGIT
        }]);
        assert!(encode_wbmp_ext(16, 1, &bits, Some(&bad), false).is_ok());
        let err = encode_wbmp_ext(16, 1, &bits, Some(&bad), true).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn ext_bitfield00_roundtrips() {
        let bits = [0x42];
        let region = ExtFields::Bitfield00(vec![0x01, 0x7F, 0x00]);
        let encoded = encode_wbmp_ext(8, 1, &bits, Some(&region), false).unwrap();
        let decoded = parse_wbmp_ext(&encoded).unwrap();
        assert_eq!(decoded.image.planes[0].data, bits);
        assert_eq!(decoded.ext_fields, Some(region));
    }

    #[test]
    fn ext_reserved01_and_reserved10_roundtrip() {
        // The Type-01 / Type-10 single-reserved-octet regions must survive
        // an encode_wbmp_ext → parse_wbmp_ext round trip on their own
        // variant (only their *error* path — a wrong plane length — was
        // previously covered). The FixHeaderField type bits carry the
        // variant selection, so a Reserved01 must not decode as Reserved10.
        let bits = [0x3C];
        for region in [ExtFields::Reserved01(0x5A), ExtFields::Reserved10(0xA5)] {
            let encoded = encode_wbmp_ext(8, 1, &bits, Some(&region), false).unwrap();
            let decoded = parse_wbmp_ext(&encoded).unwrap();
            assert_eq!(decoded.image.width, 8);
            assert_eq!(decoded.image.height, 1);
            assert_eq!(decoded.image.planes[0].data, bits);
            assert_eq!(decoded.ext_fields, Some(region));
        }
    }

    #[test]
    fn ext_strict_writer_matches_lax_for_in_class_fields() {
        // For ExtFields that already satisfy the §4.4.3 character classes,
        // the strict and lax writers must emit byte-identical streams —
        // the strict path only ever *rejects* out-of-class parameters, it
        // never changes the wire form of a conformant one.
        let bits = [0xF0, 0x0F];
        let regions = [
            ExtFields::Bitfield00(vec![0x01, 0x40, 0x00]),
            ExtFields::Reserved01(0x11),
            ExtFields::Reserved10(0x22),
            ExtFields::ParameterPairs11(vec![
                Parameter::new(b"id".to_vec(), b"Val9").unwrap(),
                Parameter::new(b"k".to_vec(), b"0").unwrap(),
            ]),
        ];
        for region in regions {
            let lax = encode_wbmp_ext(16, 1, &bits, Some(&region), false).unwrap();
            let strict = encode_wbmp_ext(16, 1, &bits, Some(&region), true).unwrap();
            assert_eq!(lax, strict, "strict == lax for in-class {region:?}");
            // And both decode back to the same ExtFields.
            assert_eq!(parse_wbmp_ext(&strict).unwrap().ext_fields, Some(region));
        }
    }

    #[test]
    fn ext_rejects_wrong_plane_length() {
        let region = ExtFields::Reserved01(0x5A);
        // 16×1 needs a 2-byte plane; pass 1 byte.
        let err = encode_wbmp_ext(16, 1, &[0x00], Some(&region), false).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn ext_rejects_zero_dimension() {
        let err = encode_wbmp_ext(0, 1, &[], None, false).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn frames_single_frame_matches_encode_wbmp() {
        // A one-element frame list must produce byte-identical output to
        // encode_wbmp with the same plane (the non-animated equivalence).
        let bits = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];
        let single = encode_wbmp(8, 8, &bits).unwrap();
        let frames = encode_wbmp_frames(8, 8, &[&bits]).unwrap();
        assert_eq!(single, frames);
    }

    #[test]
    fn frames_roundtrip_three_frames() {
        // Main image + 2 animated sub-images, 11×3 (stride 2) to also
        // exercise the padding-tail path. Decoding must recover all three
        // planes in stream order.
        let f0 = [0xAA, 0xA0, 0xF0, 0xF0, 0x00, 0x00];
        let f1 = [0x55, 0x40, 0x0F, 0x00, 0xFF, 0xE0];
        let f2 = [0xFF, 0xE0, 0x80, 0x00, 0x42, 0x40];
        let buf = encode_wbmp_frames(11, 3, &[&f0, &f1, &f2]).unwrap();

        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.width, 11);
        assert_eq!(anim.height, 3);
        assert!(anim.is_animated());
        assert_eq!(anim.animated_count(), 2);
        assert_eq!(anim.frames.len(), 3);
        assert_eq!(anim.frames[0].data, f0);
        assert_eq!(anim.frames[1].data, f1);
        assert_eq!(anim.frames[2].data, f2);
        assert_eq!(anim.frames[0].stride, 2);

        // The main image alone still decodes via the single-frame path,
        // landing byte-identically on frame 0.
        let main = parse_wbmp(&buf).unwrap();
        assert_eq!(main.planes[0].data, f0);
    }

    #[test]
    fn frames_max_animated_accepted() {
        // 1 main + 15 animated = 16 frames is the §4.5.1 maximum.
        let plane = [0x80u8]; // 1×1
        let frames: Vec<&[u8]> = (0..1 + MAX_ANIMATED_IMAGES).map(|_| &plane[..]).collect();
        let buf = encode_wbmp_frames(1, 1, &frames).unwrap();
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 1 + MAX_ANIMATED_IMAGES);
        assert_eq!(anim.animated_count(), MAX_ANIMATED_IMAGES);
    }

    #[test]
    fn frames_too_many_rejected() {
        // 17 frames (1 main + 16 animated) exceeds the §4.5.1 cap.
        let plane = [0x80u8];
        let frames: Vec<&[u8]> = (0..2 + MAX_ANIMATED_IMAGES).map(|_| &plane[..]).collect();
        let err = encode_wbmp_frames(1, 1, &frames).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn frames_empty_rejected() {
        let err = encode_wbmp_frames(4, 4, &[]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn frames_zero_dim_rejected() {
        assert!(encode_wbmp_frames(0, 1, &[&[]]).is_err());
        assert!(encode_wbmp_frames(1, 0, &[&[]]).is_err());
    }

    #[test]
    fn frames_wrong_size_frame_rejected() {
        // 8×8 needs 8 bytes per frame; the second frame is short.
        let good = [0u8; 8];
        let bad = [0u8; 7];
        let err = encode_wbmp_frames(8, 8, &[&good, &bad]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn roundtrip_8x8_pattern() {
        // 8×8 checkerboard: alternating 0xAA / 0x55 byte rows.
        let bits = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];
        let buf = encode_wbmp(8, 8, &bits).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, bits);
    }

    #[test]
    fn roundtrip_padded_dimension() {
        // 11×3 — stride = 2 bytes, total 6 body bytes.
        let bits = [
            0b1010_1010,
            0b1010_0000, // row 0
            0b1111_0000,
            0b1111_0000, // row 1
            0b0000_0000,
            0b0000_0000, // row 2 (all black)
        ];
        let buf = encode_wbmp(11, 3, &bits).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, bits);
    }

    #[test]
    fn encode_rejects_short_buffer() {
        // 16×1 needs 2 bytes; pass 1.
        let err = encode_wbmp(16, 1, &[0xFF]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn encode_rejects_zero_dim() {
        assert!(encode_wbmp(0, 1, &[]).is_err());
        assert!(encode_wbmp(1, 0, &[]).is_err());
    }

    #[test]
    fn threshold_helper_simple() {
        // 4×1 grayscale [255, 200, 50, 0]; threshold 128 → bits 1,1,0,0
        // → packed 0b1100_0000 = 0xC0.
        let buf = encode_wbmp_from_threshold(4, 1, &[255, 200, 50, 0], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, [0xC0]);
    }

    #[test]
    fn threshold_helper_threshold_at_boundary() {
        // value == threshold counts as white per spec text "≥".
        let buf = encode_wbmp_from_threshold(2, 1, &[128, 127], 128).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].data[0], 0b1000_0000);
    }

    #[test]
    fn threshold_helper_rejects_wrong_size() {
        let err = encode_wbmp_from_threshold(3, 1, &[0, 0], 128).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn dither_helper_pure_black_and_white_pass_through() {
        // Saturated inputs have zero residual to diffuse, so dither
        // and threshold-at-128 must agree byte-for-byte.
        let gray = [255u8, 255, 255, 255, 0, 0, 0, 0];
        let dith = encode_wbmp_from_dither(8, 1, &gray).unwrap();
        let thr = encode_wbmp_from_threshold(8, 1, &gray, 128).unwrap();
        assert_eq!(dith, thr);

        let img = parse_wbmp(&dith).unwrap();
        assert_eq!(img.planes[0].data, [0b1111_0000]);
    }

    #[test]
    fn dither_helper_zero_dim_rejected() {
        assert!(encode_wbmp_from_dither(0, 1, &[]).is_err());
        assert!(encode_wbmp_from_dither(1, 0, &[]).is_err());
    }

    #[test]
    fn dither_helper_rejects_wrong_size() {
        let err = encode_wbmp_from_dither(3, 1, &[0, 0]).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn dither_helper_preserves_average_on_flat_midtone() {
        // A 32×32 flat patch of value 128 should quantise to roughly
        // half white / half black under Floyd–Steinberg. With a hard
        // threshold-at-128 it'd be 100% white; dither must do
        // measurably better.
        let w = 32u32;
        let h = 32u32;
        let gray = vec![128u8; (w * h) as usize];
        let buf = encode_wbmp_from_dither(w, h, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let ones: u32 = img.planes[0].data.iter().map(|b| b.count_ones()).sum();
        // 32×32 = 1024 bits. Average 128/255 ≈ 0.50196 ≈ 514 white
        // bits would be perfect; allow a generous ±5% band to absorb
        // boundary clamping at the row ends.
        let total = w * h;
        let lo = total * 45 / 100;
        let hi = total * 55 / 100;
        assert!(
            (lo..=hi).contains(&ones),
            "dither produced {ones} white bits of {total}; expected {lo}..={hi}"
        );
    }

    #[test]
    fn dither_helper_roundtrips_width_with_padding() {
        // 11×3 — exercises the stride=2 padding-tail path the
        // threshold helper also handles.
        let gray = [
            255u8, 200, 50, 0, 255, 200, 50, 0, 128, 64, 192, //
            64, 200, 50, 255, 0, 50, 200, 64, 192, 128, 0, //
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let buf = encode_wbmp_from_dither(11, 3, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.width, 11);
        assert_eq!(img.height, 3);
        assert_eq!(img.planes[0].stride, 2);
        // Padding bits in the last byte of every row must be zero
        // (low 5 bits of each row's second byte).
        for y in 0..3 {
            let last = y * 2 + 1;
            assert_eq!(
                img.planes[0].data[last] & 0b0001_1111,
                0,
                "row {y} padding bits non-zero"
            );
        }
    }

    #[test]
    fn dither_helper_full_byte_plus_tail_bits() {
        // 11×1 grayscale: a saturated checkerboard 255/0/255/.../255.
        // Saturated inputs propagate zero residual under
        // Floyd-Steinberg, so the dither output must equal what a
        // bit-by-bit reference (set bit `7 - (x % 8)` of byte `x / 8`
        // when gray[x] >= 128) produces. This locks the r225
        // accumulator-flush pack against any future change that
        // accidentally drops a bit on the byte boundary or leaves
        // padding bits non-zero.
        let gray = [255u8, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255];
        let buf = encode_wbmp_from_dither(11, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        // Expected pack: bits 1,0,1,0,1,0,1,0 (byte 0) + 1,0,1 in
        // bits 7,6,5 of byte 1 (padding bits 4..0 are zero).
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, [0b1010_1010, 0b1010_0000]);
    }

    #[test]
    fn dither_helper_byte_boundary_padding_stays_zero() {
        // 9×1 grayscale: one full byte (bits 7..0) + one tail bit in
        // bit 7 of byte 1. The remaining low 7 bits of byte 1 are
        // padding and must be zero.
        let gray = [255u8; 9];
        let buf = encode_wbmp_from_dither(9, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        assert_eq!(img.planes[0].stride, 2);
        assert_eq!(img.planes[0].data, [0b1111_1111, 0b1000_0000]);
    }

    #[test]
    fn dither_helper_horizontal_ramp_is_balanced() {
        // 64-pixel left-to-right ramp from 0 to 255. Half above
        // 128, half below before dithering; the diffused output
        // should be close to half-and-half overall.
        let w = 64u32;
        let mut gray = Vec::with_capacity(w as usize);
        for x in 0..w {
            gray.push(((x * 255) / (w - 1)) as u8);
        }
        let buf = encode_wbmp_from_dither(w, 1, &gray).unwrap();
        let img = parse_wbmp(&buf).unwrap();
        let ones: u32 = img.planes[0].data.iter().map(|b| b.count_ones()).sum();
        // Average grayscale of the ramp is ≈ 127.5 / 255 ≈ 50%; the
        // 1-row pass has nowhere to diffuse vertically so a ±10%
        // band absorbs the residual-at-EOL clamping.
        assert!(
            (24..=40).contains(&ones),
            "ramp dithered to {ones} of 64 white bits"
        );
    }
}
