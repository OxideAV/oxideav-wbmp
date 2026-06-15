//! WBMP Type-0 decoder.
//!
//! Parses the [`crate::header::Header`] then copies the packed
//! 1-bit-per-pixel plane verbatim — the on-disk byte layout already
//! matches the [`WbmpPixelFormat::MonoWhite`] convention
//! (MSB-first, 1 = white) so no per-pixel transform is required.
//!
//! With the default `registry` feature on, the gated `WbmpDecoder` trait
//! impl wraps [`parse_wbmp`] for the `oxideav_core::Decoder` surface.

use crate::error::{Result, WbmpError};
use crate::ext::ExtFields;
use crate::header::{parse_header, parse_header_ext, parse_header_strict};
use crate::image::{PlaneLayout, WbmpImage, WbmpPixelFormat, WbmpPlane};
use crate::limits::WbmpLimits;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. One packet per whole
/// WBMP file; one frame per packet.
///
/// The caller can opt into [`oxideav_core::PixelFormat::MonoBlack`]
/// by setting `params.pixel_format = Some(PixelFormat::MonoBlack)` on
/// the [`CodecParameters`] passed into the registry — the decoder
/// will perform the polarity flip + padding-bit mask in-place during
/// decode. Any other value (including `None`) keeps the on-disk
/// [`PixelFormat::MonoWhite`] polarity.
#[cfg(feature = "registry")]
pub fn make_decoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    let target = match params.pixel_format {
        Some(PixelFormat::MonoBlack) => WbmpPixelFormat::MonoBlack,
        _ => WbmpPixelFormat::MonoWhite,
    };
    Ok(Box::new(WbmpDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
        target,
    }))
}

#[cfg(feature = "registry")]
struct WbmpDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
    target: WbmpPixelFormat,
}

#[cfg(feature = "registry")]
impl Decoder for WbmpDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_wbmp_as(&packet.data, self.target)?;
        self.pending = Some(image_to_video_frame(image));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
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

#[cfg(feature = "registry")]
fn image_to_video_frame(image: WbmpImage) -> VideoFrame {
    VideoFrame {
        pts: image.pts,
        planes: image
            .planes
            .into_iter()
            .map(|p| VideoPlane {
                stride: p.stride,
                data: p.data,
            })
            .collect(),
    }
}

/// Maximum number of animated sub-images that may follow the main
/// image in a WBMP stream (WAP-237 §4.2, §4.5.1: "The WBMP image can
/// have at most 15 animated images following the main image").
///
/// The total frame count returned by [`parse_wbmp_frames`] is therefore
/// at most `1 + MAX_ANIMATED_IMAGES == 16` (the main image plus up to 15
/// animated sub-images).
pub const MAX_ANIMATED_IMAGES: usize = 15;

/// A decoded WBMP stream including any animated sub-images that follow
/// the main image (WAP-237 §4.2 / §4.5.1).
///
/// The §4.2 BNF is `Image-data = Main-image 0*15Animated-image`, with
/// `Animated-image = *byte` "Bitmap formed according to image data
/// structure specified by the TypeField". For WBMP Type 0 that means
/// every animated sub-image is an identically-dimensioned packed
/// 1-bit-per-pixel plane (no per-frame header — the single header
/// `Width`/`Height` govern all frames), so each occupies exactly
/// `stride * height` bytes. The §4.5.1 cap is 15 animated images, so
/// `frames` holds 1..=16 planes: index 0 is the main image, indices
/// 1.. are the animated sub-images in stream order.
///
/// Returned by [`parse_wbmp_frames`] / [`parse_wbmp_frames_with_limits`].
/// "It is User Agent dependent how those animated images are processed"
/// (§4.5.1) — this crate surfaces the raw frame planes and leaves
/// presentation timing to the caller, since WAP-237 defines no animation
/// timing parameters.
#[derive(Debug, Clone)]
pub struct WbmpAnimation {
    /// Picture width in pixels (shared by every frame).
    pub width: u32,
    /// Picture height in pixels (shared by every frame).
    pub height: u32,
    /// Pixel layout the planes carry — always [`WbmpPixelFormat::MonoWhite`]
    /// from this entry point (the on-disk polarity).
    pub pixel_format: WbmpPixelFormat,
    /// One packed plane per frame: index 0 is the main image, indices
    /// `1..` are the animated sub-images in stream order. Always at
    /// least one element (the main image); at most
    /// `1 + MAX_ANIMATED_IMAGES`.
    pub frames: Vec<WbmpPlane>,
}

impl WbmpAnimation {
    /// Number of animated sub-images following the main image (i.e.
    /// `frames.len() - 1`). `0` for a single-frame (non-animated) WBMP.
    pub fn animated_count(&self) -> usize {
        self.frames.len() - 1
    }

    /// `true` when the stream carries at least one animated sub-image.
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// View the main image (frame 0) as a standalone [`WbmpImage`],
    /// discarding any animated sub-images. Equivalent to what
    /// [`parse_wbmp`] returns for the same input.
    pub fn main_image(&self) -> WbmpImage {
        WbmpImage {
            width: self.width,
            height: self.height,
            pixel_format: self.pixel_format,
            planes: vec![self.frames[0].clone()],
            pts: None,
        }
    }
}

/// Decode a complete WBMP file (Type 0 only) into a [`WbmpImage`]
/// using the default [`WbmpLimits`].
///
/// Returns:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero (no
///   other type is defined by WAP-237 normatively or widely
///   deployed).
/// * [`WbmpError::InvalidData`] for truncated headers, MBI overflow,
///   or pixel-data shorter than what `width × height` requires.
/// * [`WbmpError::LimitExceeded`] if the header dimensions or
///   computed pixel-data size exceeds the default
///   [`WbmpLimits`] — see [`parse_wbmp_with_limits`] for callers
///   that need to allow larger images.
pub fn parse_wbmp(input: &[u8]) -> Result<WbmpImage> {
    parse_wbmp_with_limits(input, &WbmpLimits::default())
}

/// Decode a WBMP file with caller-supplied resource limits. Identical
/// to [`parse_wbmp`] except the [`WbmpLimits`] are taken from
/// `limits` instead of [`WbmpLimits::default`].
///
/// Use [`WbmpLimits::unbounded`] for trusted local input where the
/// decoder should allocate whatever the header asks for; otherwise
/// keep the defaults or tighten them further for your environment.
pub fn parse_wbmp_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    parse_wbmp_inner(input, limits, false)
}

/// Strict variant of [`parse_wbmp`]. Identical except the header is
/// parsed with [`parse_header_strict`] — the `FixedHeader` byte is
/// required to be exactly `0x00`. Any other value raises
/// [`WbmpError::InvalidData`].
///
/// Use this entry point when the caller wants to reject malformed /
/// non-conformant Type-0 files at the wire-format level rather than
/// silently accept a byte the spec does not currently assign meaning
/// to. The lax [`parse_wbmp`] is forward-compatible with hypothetical
/// Type-0 extensions; this one is not.
pub fn parse_wbmp_strict(input: &[u8]) -> Result<WbmpImage> {
    parse_wbmp_strict_with_limits(input, &WbmpLimits::default())
}

/// Strict variant of [`parse_wbmp_with_limits`]. Same strict
/// `FixedHeader == 0x00` enforcement as [`parse_wbmp_strict`],
/// combined with caller-supplied [`WbmpLimits`].
pub fn parse_wbmp_strict_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImage> {
    parse_wbmp_inner(input, limits, true)
}

/// Decoded WBMP image paired with any parsed extension headers
/// (`ExtFields`, WAP-237 §4.4.1–§4.4.3) found between the
/// `FixHeaderField` and the dimensions.
///
/// Returned by [`parse_wbmp_ext`] / [`parse_wbmp_ext_with_limits`]. In a
/// conformant **Type 0** file `ext_fields` is always `None` (§4.5.1
/// fixes the `FixHeaderField` at `0x00`); the field is `Some` only for a
/// non-conformant Type-0 producer that emitted extension headers.
#[derive(Debug, Clone)]
pub struct WbmpImageExt {
    /// The decoded main image.
    pub image: WbmpImage,
    /// The parsed `ExtFields` region, or `None` when the
    /// `FixHeaderField` presence flag was clear.
    pub ext_fields: Option<ExtFields>,
}

/// Decode a WBMP file, honouring the `FixHeaderField` extension-header
/// presence flag, using the default [`WbmpLimits`].
///
/// Unlike [`parse_wbmp`] — which treats the byte after the `TypeField`
/// as a fixed one-octet `FixedHeader` and reads `Width` immediately
/// after it — this entry point parses the header through
/// [`parse_header_ext`]. When the `FixHeaderField` bit-7 presence flag
/// is set, the `ExtFields` region (whose layout is selected by bits 6-5)
/// is consumed and surfaced before `Width`/`Height` are read.
///
/// For a conformant Type-0 file (`FixHeaderField == 0x00`) the decoded
/// image is byte-for-byte identical to [`parse_wbmp`] and `ext_fields`
/// comes back `None`. The value of this path is decoding a
/// non-conformant Type-0 file that carries extension headers: [`parse_wbmp`]
/// would mis-read the first `ExtField` octet as the `Width` MBI, whereas
/// this skips the `ExtFields` and lands on the real dimensions.
pub fn parse_wbmp_ext(input: &[u8]) -> Result<WbmpImageExt> {
    parse_wbmp_ext_with_limits(input, &WbmpLimits::default())
}

/// Extension-header-aware decode with caller-supplied [`WbmpLimits`].
/// See [`parse_wbmp_ext`] for the extension-header semantics and
/// [`parse_wbmp_with_limits`] for the limits semantics.
pub fn parse_wbmp_ext_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpImageExt> {
    let header = parse_header_ext(input)?;
    let image = decode_body(
        input,
        header.width,
        header.height,
        header.data_offset,
        limits,
    )?;
    Ok(WbmpImageExt {
        image,
        ext_fields: header.ext_fields,
    })
}

/// Decode a WBMP stream into its main image **and** any animated
/// sub-images that follow it (WAP-237 §4.2 / §4.5.1), using the default
/// [`WbmpLimits`].
///
/// WAP-237 defines `Image-data = Main-image 0*15Animated-image`: after
/// the single four-field header, the main image's `stride * height`
/// packed bytes are followed by 0..15 further packed bitmaps of the
/// **same** dimensions (there is no per-frame header — the lone header
/// `Width`/`Height` apply to every frame). This entry point reads the
/// main image, then greedily consumes each following `stride * height`
/// chunk as an animated sub-image until either fewer than one full frame
/// of bytes remain or the §4.5.1 cap of [`MAX_ANIMATED_IMAGES`] animated
/// frames is reached.
///
/// A trailing run shorter than one full frame is treated as ignorable
/// padding (matching the single-frame [`parse_wbmp`], which already
/// tolerates trailing bytes past the main image). A conformant
/// single-frame WBMP therefore yields a [`WbmpAnimation`] whose `frames`
/// holds exactly one plane, byte-identical to [`parse_wbmp`]'s output.
///
/// Errors mirror [`parse_wbmp`]: [`WbmpError::Unsupported`] for a
/// non-zero Type, [`WbmpError::InvalidData`] for a truncated header /
/// MBI overflow / a main image shorter than `stride * height`, and
/// [`WbmpError::LimitExceeded`] when the per-frame dimensions or
/// pixel-data size exceed `limits`.
pub fn parse_wbmp_frames(input: &[u8]) -> Result<WbmpAnimation> {
    parse_wbmp_frames_with_limits(input, &WbmpLimits::default())
}

/// Animated-aware decode with caller-supplied [`WbmpLimits`]. See
/// [`parse_wbmp_frames`] for the animation semantics and
/// [`parse_wbmp_with_limits`] for the limits semantics.
///
/// The [`WbmpLimits::max_pixel_bytes`] cap is applied **per frame**
/// (each animated sub-image is the same size as the main image), so a
/// stream cannot exceed `max_pixel_bytes` for any single plane regardless
/// of how many frames it carries. The §4.5.1 frame-count ceiling of
/// [`MAX_ANIMATED_IMAGES`] bounds the total work independently of the
/// byte budget.
pub fn parse_wbmp_frames_with_limits(input: &[u8], limits: &WbmpLimits) -> Result<WbmpAnimation> {
    let header = parse_header(input)?;
    decode_frames(
        input,
        header.width,
        header.height,
        header.data_offset,
        limits,
    )
}

/// Decode the main image plus any trailing animated sub-images, sharing
/// the header `(width, height, data_offset)`. The main image's limit
/// checks + plane-layout computation reuse [`decode_body`]; each animated
/// sub-image is a verbatim `stride * height` chunk read from the bytes
/// after the previous frame.
fn decode_frames(
    input: &[u8],
    width: u32,
    height: u32,
    data_offset: usize,
    limits: &WbmpLimits,
) -> Result<WbmpAnimation> {
    // The main image goes through decode_body, which applies every limit
    // check (dimensions, pixel-byte cap, overflow guard) and the
    // truncation check, then copies the first plane verbatim.
    let main = decode_body(input, width, height, data_offset, limits)?;

    // After decode_body succeeds, the layout is known-good and the main
    // image consumed `layout.total_bytes` bytes starting at data_offset.
    let layout =
        PlaneLayout::new(width, height).map_err(|msg| WbmpError::invalid(msg.to_string()))?;

    let mut frames = Vec::with_capacity(1);
    frames.push(main.planes.into_iter().next().expect("main image plane"));

    // A zero-byte plane (width or height collapses to a 0-byte layout)
    // can never appear here — parse_header rejects zero dimensions — so
    // total_bytes >= 1 and the loop below always advances.
    let mut offset = data_offset.saturating_add(layout.total_bytes);
    while frames.len() <= MAX_ANIMATED_IMAGES {
        let remaining = input.len().saturating_sub(offset);
        if remaining < layout.total_bytes {
            // Fewer than one full animated frame remains — treat the tail
            // as ignorable padding (same posture as parse_wbmp toward
            // trailing bytes past the main image).
            break;
        }
        let end = offset + layout.total_bytes;
        frames.push(WbmpPlane {
            stride: layout.stride,
            data: input[offset..end].to_vec(),
        });
        offset = end;
    }

    Ok(WbmpAnimation {
        width,
        height,
        pixel_format: WbmpPixelFormat::MonoWhite,
        frames,
    })
}

fn parse_wbmp_inner(input: &[u8], limits: &WbmpLimits, strict: bool) -> Result<WbmpImage> {
    let header = if strict {
        parse_header_strict(input)?
    } else {
        parse_header(input)?
    };
    decode_body(
        input,
        header.width,
        header.height,
        header.data_offset,
        limits,
    )
}

/// Decode the main image data given an already-parsed `(width, height,
/// data_offset)`. Shared by the plain header path
/// ([`parse_wbmp_inner`]) and the extension-header-aware path
/// ([`parse_wbmp_ext_with_limits`]) so the limit checks, plane-layout
/// computation and verbatim row copy stay in one place.
///
/// `data_offset` is the byte index of the first main-image-data octet,
/// i.e. immediately past `Height` (and past any `ExtFields` the caller
/// already consumed).
fn decode_body(
    input: &[u8],
    width: u32,
    height: u32,
    data_offset: usize,
    limits: &WbmpLimits,
) -> Result<WbmpImage> {
    if width > limits.max_width {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: width {} exceeds max_width {}",
            width, limits.max_width
        )));
    }
    if height > limits.max_height {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: height {} exceeds max_height {}",
            height, limits.max_height
        )));
    }

    let layout =
        PlaneLayout::new(width, height).map_err(|msg| WbmpError::invalid(msg.to_string()))?;

    if layout.total_bytes > limits.max_pixel_bytes {
        return Err(WbmpError::limit_exceeded(format!(
            "WBMP: pixel-data size {} exceeds max_pixel_bytes {}",
            layout.total_bytes, limits.max_pixel_bytes
        )));
    }

    let body = &input[data_offset..];
    if body.len() < layout.total_bytes {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel data truncated (need {} bytes, got {})",
            layout.total_bytes,
            body.len()
        )));
    }

    // Byte layout matches our plane format directly — copy verbatim.
    // We allow trailing bytes past `layout.total_bytes` (some encoders
    // pad to even byte boundaries); we just drop them.
    let data = body[..layout.total_bytes].to_vec();

    Ok(WbmpImage {
        width,
        height,
        pixel_format: WbmpPixelFormat::MonoWhite,
        planes: vec![WbmpPlane {
            stride: layout.stride,
            data,
        }],
        pts: None,
    })
}

/// Decode a WBMP file into the requested [`WbmpPixelFormat`] using the
/// default [`WbmpLimits`].
///
/// Behaves identically to [`parse_wbmp`] when `target` is
/// [`WbmpPixelFormat::MonoWhite`]. When `target` is
/// [`WbmpPixelFormat::MonoBlack`] every bit is inverted before being
/// returned, and any padding bits in the last byte of every row are
/// masked back to zero so they stay distinguishable from real black
/// pixels — matching the symmetric convention the encoder uses when it
/// accepts a `MonoBlack` plane.
///
/// The wire format never changes — this is a decode-side convenience
/// only, equivalent to (but cheaper than) parsing first and walking
/// the plane afterwards because the inversion happens in-place during
/// the decode-time row copy.
pub fn parse_wbmp_as(input: &[u8], target: WbmpPixelFormat) -> Result<WbmpImage> {
    parse_wbmp_as_with_limits(input, target, &WbmpLimits::default())
}

/// Decode a WBMP file into the requested [`WbmpPixelFormat`] with
/// caller-supplied [`WbmpLimits`].
///
/// See [`parse_wbmp_as`] for the polarity semantics and
/// [`parse_wbmp_with_limits`] for the limits semantics.
pub fn parse_wbmp_as_with_limits(
    input: &[u8],
    target: WbmpPixelFormat,
    limits: &WbmpLimits,
) -> Result<WbmpImage> {
    let mut image = parse_wbmp_with_limits(input, limits)?;
    convert_plane_polarity(&mut image, target);
    Ok(image)
}

/// Apply an in-place polarity transform to bring the on-disk
/// `MonoWhite` plane returned by [`parse_wbmp_with_limits`] into the
/// caller's requested [`WbmpPixelFormat`].
///
/// No-op when `target == MonoWhite`. For `MonoBlack` we invert every
/// payload byte and then zero out the padding bits in the last byte of
/// every row (so a `1`-bit in the plane unambiguously means "black",
/// rather than "either black or unused padding bit").
fn convert_plane_polarity(image: &mut WbmpImage, target: WbmpPixelFormat) {
    if image.pixel_format == target {
        return;
    }
    match target {
        WbmpPixelFormat::MonoWhite => {
            // The decode path always emits MonoWhite verbatim — this
            // branch only fires if the caller passes a MonoBlack image
            // back through us. Same masking + inversion logic round-
            // trips cleanly to MonoWhite.
            invert_plane_in_place(image);
            image.pixel_format = WbmpPixelFormat::MonoWhite;
        }
        WbmpPixelFormat::MonoBlack => {
            invert_plane_in_place(image);
            image.pixel_format = WbmpPixelFormat::MonoBlack;
        }
    }
}

/// Flip every bit of the single packed plane in `image` and re-zero
/// the padding bits in the last byte of every row.
///
/// Padding handling matters: after inverting, what used to be a
/// row's trailing-zero padding becomes a trailing-one run that
/// callers iterating bit-by-bit would mistake for real foreground
/// pixels. We mask it back to zero so the plane stays well-formed
/// regardless of polarity.
///
/// The per-row mask comes from the [`PlaneLayout`] typed primitive
/// (`last_byte_pad_mask`), which is the same byte the encoder's
/// `MonoBlack` ingress branch uses — keeping the two sites in sync on
/// the masking convention.
fn invert_plane_in_place(image: &mut WbmpImage) {
    let plane = &mut image.planes[0];
    for b in plane.data.iter_mut() {
        *b = !*b;
    }
    // `PlaneLayout::new` only fails on usize overflow; we already
    // allocated the plane bytes for this (width, height) so by
    // construction the layout is computable.
    let layout = match PlaneLayout::new(image.width, image.height) {
        Ok(l) => l,
        Err(_) => return,
    };
    // `last_byte_pad_mask` is 0xFF when there are no padding bits to
    // mask — in that case the per-row AND below is a no-op, but
    // skipping the loop also skips the per-row branch + index
    // computation. The width=0 case still has stride=0 and would
    // produce a malformed `last = -1` index without this guard.
    if layout.last_byte_pad_mask != 0xFF && layout.stride > 0 {
        for y in 0..layout.height as usize {
            let last = y * layout.stride + (layout.stride - 1);
            if last < plane.data.len() {
                plane.data[last] &= layout.last_byte_pad_mask;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::write_header;
    use crate::mbi::write_mbi_u32;

    #[test]
    fn parse_minimal_1x1_white() {
        // 1×1 image, single pixel = white (bit 7 of byte 0 set).
        let mut buf = Vec::new();
        write_header(1, 1, &mut buf);
        buf.push(0b1000_0000);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
        assert_eq!(image.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(image.planes.len(), 1);
        assert_eq!(image.planes[0].stride, 1);
        assert_eq!(image.planes[0].data, [0b1000_0000]);
    }

    #[test]
    fn parse_padded_row() {
        // 11×1: row needs 2 bytes (16 bits, last 5 padding).
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.planes[0].stride, 2);
        assert_eq!(image.planes[0].data, [0b1010_1100, 0b1110_0000]);
    }

    #[test]
    fn parse_truncated_pixel_data_errors() {
        // Header says 16×1 (2 bytes per row) but only 1 body byte
        // present.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf);
        buf.push(0xFF);
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)));
    }

    #[test]
    fn parse_rejects_unknown_type() {
        // Type=1 — not standardised.
        let buf = [
            0x01u8, 0x00, 0x08, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)));
    }

    #[test]
    fn trailing_bytes_after_pixel_data_are_ignored() {
        // 8×1 (1 body byte) + 3 garbage bytes afterwards.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0x55);
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE]);
        let image = parse_wbmp(&buf).unwrap();
        assert_eq!(image.planes[0].data, [0x55]);
    }

    // --- Hardening tests against malformed / adversarial input. ---

    #[test]
    fn rejects_oversized_width_under_default_limits() {
        // Width MBI = 0x82_80_00 (= 32768) — twice the default
        // max_width of 16384. Decoder must error before touching the
        // allocator.
        let buf = [
            0x00u8, 0x00, // Type=0, FixedHeader
            0x82, 0x80, 0x00, // Width = 32768
            0x01, // Height = 1
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn rejects_oversized_height_under_default_limits() {
        // Width=1, Height=32768.
        let buf = [
            0x00u8, 0x00, // Type=0, FixedHeader
            0x01, // Width = 1
            0x82, 0x80, 0x00, // Height = 32768
        ];
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn rejects_pixel_byte_blowup_under_default_limits() {
        // 16000 × 16000 sneaks under max_width/max_height (both 16384)
        // but width*height/8 = 32 MB blows past max_pixel_bytes
        // (8 MiB).
        let mut buf = Vec::new();
        write_header(16000, 16000, &mut buf);
        // Don't append any pixel data — limit check fires first.
        let err = parse_wbmp(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn unbounded_limits_admit_larger_image() {
        // A 20000 × 1 (only 2500 body bytes) blows the default width
        // cap but is fine with WbmpLimits::unbounded() and the cheap
        // body.
        let mut buf = Vec::new();
        write_header(20000, 1, &mut buf);
        buf.extend_from_slice(&[0u8; 2500]);
        assert!(matches!(
            parse_wbmp(&buf).unwrap_err(),
            WbmpError::LimitExceeded(_)
        ));
        let img = parse_wbmp_with_limits(&buf, &WbmpLimits::unbounded()).unwrap();
        assert_eq!(img.width, 20000);
        assert_eq!(img.height, 1);
        assert_eq!(img.planes[0].data.len(), 2500);
    }

    #[test]
    fn custom_limits_can_be_tighter_than_defaults() {
        // Caller wants max 64-pixel images. A 65×1 must be rejected.
        let mut buf = Vec::new();
        write_header(65, 1, &mut buf);
        buf.push(0u8);
        buf.push(0u8);
        let tight = WbmpLimits {
            max_width: 64,
            ..WbmpLimits::default()
        };
        let err = parse_wbmp_with_limits(&buf, &tight).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn fuzz_short_byte_prefixes_never_panic() {
        // Every 1-byte and most 2-byte sequences either parse (small
        // images) or return a tidy error; none should panic. We feed
        // every possible 2-byte prefix, then enough random-ish
        // 3..=8-byte sequences to cover all error paths in the header
        // parser plus the body-length check.
        for a in 0u8..=255 {
            for b in 0u8..=255 {
                let _ = parse_wbmp(&[a, b]);
            }
        }
        // Random-ish coverage of slightly longer inputs — fixed seed
        // (LCG) so the test is deterministic.
        let mut seed: u64 = 0xDEAD_BEEF_CAFE_BABE;
        for _ in 0..4096 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = 1 + (seed as usize) % 20;
            let mut buf = vec![0u8; len];
            for byte in buf.iter_mut() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = seed as u8;
            }
            let _ = parse_wbmp(&buf);
        }
    }

    // --- Polarity (MonoWhite ↔ MonoBlack) decode tests. ---

    #[test]
    fn parse_as_monowhite_matches_parse_wbmp() {
        // parse_wbmp_as(MonoWhite) must be byte-for-byte identical to
        // parse_wbmp for the same input.
        let mut buf = Vec::new();
        write_header(11, 3, &mut buf);
        let body = [0xAA, 0x80, 0x55, 0xA0, 0xC3, 0x40];
        buf.extend_from_slice(&body);
        let a = parse_wbmp(&buf).unwrap();
        let b = parse_wbmp_as(&buf, WbmpPixelFormat::MonoWhite).unwrap();
        assert_eq!(a.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(b.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(a.planes[0].data, b.planes[0].data);
    }

    #[test]
    fn parse_as_monoblack_inverts_full_byte_rows() {
        // 8×1 byte-aligned row: the polarity flip is a clean `!byte`
        // with no padding to mask.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0b1010_0110);
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap();
        assert_eq!(img.pixel_format, WbmpPixelFormat::MonoBlack);
        assert_eq!(img.planes[0].stride, 1);
        assert_eq!(img.planes[0].data, [0b0101_1001]);
    }

    #[test]
    fn parse_as_monoblack_masks_padding_bits() {
        // 11×1 → stride 2, 5 padding bits in the last byte. After
        // inversion those would become five trailing `1` bits unless
        // masked. We assert they're back to zero.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        // First 11 bits (MSB-first): 1010 1100 111 — packed
        // 0xAC, then 0xE0 (with 5 padding zeros).
        buf.push(0xAC);
        buf.push(0xE0);
        let img = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap();
        // Inverted: 0x53 in byte 0; byte 1 inversion would give
        // 0x1F, but masking the 5 padding bits zeroes them → 0x00.
        assert_eq!(img.planes[0].data, [0x53, 0x00]);
    }

    #[test]
    fn parse_as_monoblack_roundtrips_through_inversion() {
        // Decoding to MonoBlack twice (via parse_as → encode → parse_as)
        // recovers the original on-disk bits exactly. We pick a width
        // with a non-trivial padding tail (159 → 4 padding bits).
        let stride = WbmpImage::row_stride(159);
        let mut bits = vec![0u8; stride * 5];
        let mut seed: u64 = 0xC0FF_EE00_BAAD_F00D;
        for byte in bits.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = seed as u8;
        }
        // Pre-zero the padding bits in the test fixture so the round
        // trip target is well-formed input → MonoWhite verbatim.
        let pad_bits = stride * 8 - 159;
        let mask: u8 = 0xFFu8 << pad_bits;
        for y in 0..5 {
            let last = y * stride + (stride - 1);
            bits[last] &= mask;
        }
        let encoded = crate::encoder::encode_wbmp(159, 5, &bits).unwrap();
        // First decode: MonoBlack (inverted with padding masked).
        let blk = parse_wbmp_as(&encoded, WbmpPixelFormat::MonoBlack).unwrap();
        assert_eq!(blk.pixel_format, WbmpPixelFormat::MonoBlack);
        // Manually invert + mask: must match the MonoBlack plane bit-
        // for-bit.
        let mut expect = bits.clone();
        for b in expect.iter_mut() {
            *b = !*b;
        }
        for y in 0..5 {
            let last = y * stride + (stride - 1);
            expect[last] &= mask;
        }
        assert_eq!(blk.planes[0].data, expect);
    }

    #[test]
    fn parse_as_monoblack_respects_limits() {
        // Limit checks fire before the polarity flip — a MonoBlack
        // decode of an over-sized header must still raise
        // LimitExceeded, not run through the inversion loop on
        // unallocated memory.
        let mut buf = Vec::new();
        write_header(16_000, 16_000, &mut buf);
        let err = parse_wbmp_as(&buf, WbmpPixelFormat::MonoBlack).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_as_with_limits_propagates_unbounded() {
        // 20000×1 (2500 body bytes) blows the default width cap but
        // passes with WbmpLimits::unbounded(), in both polarities.
        let mut buf = Vec::new();
        write_header(20_000, 1, &mut buf);
        buf.extend_from_slice(&[0xFFu8; 2500]);
        let lim = WbmpLimits::unbounded();
        let w = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoWhite, &lim).unwrap();
        let b = parse_wbmp_as_with_limits(&buf, WbmpPixelFormat::MonoBlack, &lim).unwrap();
        assert_eq!(w.planes[0].data, vec![0xFFu8; 2500]);
        assert_eq!(b.planes[0].data, vec![0x00u8; 2500]);
    }

    // --- Strict-mode reject path (FixedHeader == 0x00 required). ---

    #[test]
    fn parse_wbmp_strict_matches_lax_on_conformant_input() {
        // Well-formed Type-0 file (FixedHeader = 0x00): the strict and
        // lax entry points must produce byte-for-byte identical
        // results.
        let mut buf = Vec::new();
        write_header(11, 2, &mut buf);
        let body = [0xAC, 0xE0, 0x53, 0x00];
        buf.extend_from_slice(&body);
        let lax = parse_wbmp(&buf).unwrap();
        let strict = parse_wbmp_strict(&buf).unwrap();
        assert_eq!(lax.width, strict.width);
        assert_eq!(lax.height, strict.height);
        assert_eq!(lax.pixel_format, strict.pixel_format);
        assert_eq!(lax.planes[0].stride, strict.planes[0].stride);
        assert_eq!(lax.planes[0].data, strict.planes[0].data);
    }

    #[test]
    fn parse_wbmp_strict_rejects_nonzero_fixed_header() {
        // Same bytes as parse_padded_row but with FixedHeader = 0xFF.
        // The lax parser still accepts it (forward-compat); the strict
        // parser must error out as InvalidData.
        let buf = [
            0x00u8, // Type = 0
            0xFF,   // FixedHeader = 0xFF (non-conformant)
            0x0B,   // Width = 11
            0x01,   // Height = 1
            0xAC, 0xE0, // 11 pixels packed (5 bits padding)
        ];
        assert!(parse_wbmp(&buf).is_ok(), "lax parser still accepts");
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_rejects_redundantly_padded_dimension_mbi() {
        // §4.3.1 shortest-encoding MUST NOT, now enforced on the full
        // strict decode path: a leading-0x80-padded Width MBI is decoded
        // fine by the lax parser but rejected by the strict one. The
        // FixedHeader is the conformant 0x00 so this isolates the MBI
        // shortest-encoding check from the FixedHeader check.
        let buf = [
            0x00u8, // Type = 0
            0x00,   // FixedHeader = 0x00 (conformant)
            0x80, 0x0B, // Width = 11, but non-minimal (leading 0x80)
            0x01, // Height = 1
            0xAC, 0xE0, // 11 pixels packed
        ];
        let lax = parse_wbmp(&buf).unwrap();
        assert_eq!(lax.width, 11);
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_with_limits_enforces_both() {
        // FixedHeader violation fires first (before the limit check)
        // when the file is also out of bounds — the strict header path
        // runs before allocation.
        let buf = [
            0x00u8, 0x01, // FixedHeader = 0x01 — strict rejects
            0x82, 0x80, 0x00, // Width = 32768 (would also be over the default cap)
            0x01, // Height = 1
        ];
        let err = parse_wbmp_strict_with_limits(&buf, &WbmpLimits::default()).unwrap_err();
        // We promise InvalidData here, not LimitExceeded — strict mode
        // wants to surface the FixedHeader violation as soon as it
        // sees it, before the limit machinery.
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_still_enforces_limits_on_conformant_header() {
        // FixedHeader = 0x00 (conformant) but dimensions blow the
        // default limit. Strict mode must still return LimitExceeded
        // — strict is an ADDITIONAL check, not a replacement.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_wbmp_strict_still_rejects_nonzero_type() {
        // Non-zero Type field must surface as Unsupported in both
        // parsers; strict mode tightens the FixedHeader check, not the
        // Type check.
        let buf = [0x01u8, 0x00, 0x08, 0x08];
        let err = parse_wbmp_strict(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    // --- parse_wbmp_ext (extension-header-aware decode) tests. ---

    #[test]
    fn parse_ext_conformant_type0_matches_plain_decode() {
        // FixHeaderField = 0x00 → no ExtFields. The ext-aware decode
        // must produce the identical image and report ext_fields None.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let plain = parse_wbmp(&buf).unwrap();
        let ext = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(ext.image.width, plain.width);
        assert_eq!(ext.image.height, plain.height);
        assert_eq!(ext.image.pixel_format, plain.pixel_format);
        assert_eq!(ext.image.planes[0].stride, plain.planes[0].stride);
        assert_eq!(ext.image.planes[0].data, plain.planes[0].data);
        assert!(ext.ext_fields.is_none());
    }

    #[test]
    fn parse_ext_decodes_image_after_parameter_pairs() {
        // Non-conformant Type-0 file carrying a Type-11 ExtFields region
        // before the dimensions. parse_wbmp would mis-read the
        // ParameterHeader octet as the Width MBI; parse_wbmp_ext must
        // skip the ExtFields and decode the real 8x1 image.
        use crate::ext::{write_ext_fields, ExtFields, Parameter};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf); // Type = 0
        buf.push(0b1110_0000); // FixHeaderField: ext follow, type 11
        let ext = ExtFields::ParameterPairs11(vec![Parameter {
            identifier: b"id".to_vec(),
            value: b"v".to_vec(),
        }]);
        write_ext_fields(&ext, &mut buf).unwrap();
        write_mbi_u32(8, &mut buf); // Width = 8
        write_mbi_u32(1, &mut buf); // Height = 1
        buf.push(0b1010_1010); // one body byte (8x1 = 1 byte/row)

        let parsed = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(parsed.image.width, 8);
        assert_eq!(parsed.image.height, 1);
        assert_eq!(parsed.image.planes[0].data, [0b1010_1010]);
        assert_eq!(parsed.image.pixel_format, WbmpPixelFormat::MonoWhite);
        assert_eq!(parsed.ext_fields, Some(ext));
    }

    #[test]
    fn parse_ext_decodes_image_after_bitfield00_chain() {
        use crate::ext::{write_ext_fields, ExtFields};
        let mut buf = Vec::new();
        write_mbi_u32(0, &mut buf); // Type = 0
        buf.push(0b1000_0000); // FixHeaderField: ext follow, type 00
        let ext = ExtFields::Bitfield00(vec![0x01, 0x42]);
        write_ext_fields(&ext, &mut buf).unwrap();
        write_mbi_u32(4, &mut buf); // Width = 4
        write_mbi_u32(2, &mut buf); // Height = 2
        buf.push(0b1100_0000); // row 0 (4px in 1 byte)
        buf.push(0b0011_0000); // row 1

        let parsed = parse_wbmp_ext(&buf).unwrap();
        assert_eq!(parsed.image.width, 4);
        assert_eq!(parsed.image.height, 2);
        assert_eq!(parsed.image.planes[0].data, [0b1100_0000, 0b0011_0000]);
        assert_eq!(parsed.ext_fields, Some(ext));
    }

    #[test]
    fn parse_ext_enforces_limits() {
        // Conformant header but dimensions blow the default cap — the
        // ext-aware path must still apply WbmpLimits via decode_body.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn parse_ext_truncated_pixel_data_errors() {
        // ExtFields parse fine, dimensions fine, but the body is short.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf); // needs 2 body bytes
        buf.push(0x00); // only 1
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn parse_ext_rejects_truncated_ext_region() {
        // FixHeaderField says ExtFields follow (type 00) but the
        // continuation bit is set with no terminating octet → the body
        // never starts. Must error, not panic.
        let buf = [
            0x00u8,      // Type = 0
            0b1000_0000, // FixHeaderField: ext follow, type 00
            0x80,        // bitfield octet with continuation bit, stream ends
        ];
        let err = parse_wbmp_ext(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    // --- Animated sub-image decode tests (WAP-237 §4.2 / §4.5.1). ---

    #[test]
    fn frames_single_image_matches_parse_wbmp() {
        // A conformant single-frame WBMP yields exactly one frame, plane
        // byte-identical to parse_wbmp.
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf);
        buf.push(0b1010_1100);
        buf.push(0b1110_0000);
        let plain = parse_wbmp(&buf).unwrap();
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 1);
        assert!(!anim.is_animated());
        assert_eq!(anim.animated_count(), 0);
        assert_eq!(anim.width, plain.width);
        assert_eq!(anim.height, plain.height);
        assert_eq!(anim.pixel_format, plain.pixel_format);
        assert_eq!(anim.frames[0].stride, plain.planes[0].stride);
        assert_eq!(anim.frames[0].data, plain.planes[0].data);
        // main_image() reproduces the single-frame view.
        let main = anim.main_image();
        assert_eq!(main.planes[0].data, plain.planes[0].data);
        assert_eq!(main.width, plain.width);
    }

    #[test]
    fn frames_decodes_main_plus_animated_subimages() {
        // 8×1 main image + two animated sub-images, each 1 body byte.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        buf.push(0b1111_0000); // main
        buf.push(0b0000_1111); // animated frame 1
        buf.push(0b1010_1010); // animated frame 2
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 3);
        assert!(anim.is_animated());
        assert_eq!(anim.animated_count(), 2);
        assert_eq!(anim.frames[0].data, [0b1111_0000]);
        assert_eq!(anim.frames[1].data, [0b0000_1111]);
        assert_eq!(anim.frames[2].data, [0b1010_1010]);
        // Frame 0 still matches the single-frame parse_wbmp (which
        // ignores the trailing animated bytes).
        let plain = parse_wbmp(&buf).unwrap();
        assert_eq!(anim.frames[0].data, plain.planes[0].data);
    }

    #[test]
    fn frames_multibyte_rows_animated() {
        // 11×2 → stride 2, total_bytes 4 per frame. Main + 1 animated.
        let mut buf = Vec::new();
        write_header(11, 2, &mut buf);
        let main = [0xAC, 0xE0, 0x53, 0x00];
        let f1 = [0x12, 0x80, 0x34, 0x40];
        buf.extend_from_slice(&main);
        buf.extend_from_slice(&f1);
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].stride, 2);
        assert_eq!(anim.frames[0].data, main);
        assert_eq!(anim.frames[1].data, f1);
    }

    #[test]
    fn frames_partial_trailing_run_is_ignored() {
        // 8×1 main + 2 full animated frames + 0 stray bytes that don't
        // make a full frame (stride*height = 1 here, so use a 11×1 image
        // where a single stray byte is < the 2-byte frame size).
        let mut buf = Vec::new();
        write_header(11, 1, &mut buf); // stride 2, frame = 2 bytes
        buf.extend_from_slice(&[0xAC, 0xE0]); // main
        buf.extend_from_slice(&[0x12, 0x80]); // animated frame 1
        buf.push(0x77); // a single stray byte — < one full frame
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].data, [0xAC, 0xE0]);
        assert_eq!(anim.frames[1].data, [0x12, 0x80]);
    }

    #[test]
    fn frames_caps_at_max_animated_images() {
        // 8×1 main + 20 animated-frame-sized chunks. The §4.5.1 cap is
        // 15 animated images, so the decoder must stop after 16 total
        // frames and ignore the rest.
        let mut buf = Vec::new();
        write_header(8, 1, &mut buf);
        for i in 0..=20u8 {
            buf.push(i);
        }
        let anim = parse_wbmp_frames(&buf).unwrap();
        assert_eq!(anim.frames.len(), 1 + MAX_ANIMATED_IMAGES);
        assert_eq!(anim.animated_count(), MAX_ANIMATED_IMAGES);
        // Frames 0..=15 carry bytes 0..=15; bytes 16..=20 are dropped.
        for (i, frame) in anim.frames.iter().enumerate() {
            assert_eq!(frame.data, [i as u8]);
        }
    }

    #[test]
    fn frames_truncated_main_image_errors() {
        // The main image itself is short — must surface InvalidData, not
        // silently return zero frames.
        let mut buf = Vec::new();
        write_header(16, 1, &mut buf); // needs 2 body bytes
        buf.push(0xFF); // only 1
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::InvalidData(_)), "{err:?}");
    }

    #[test]
    fn frames_rejects_non_zero_type() {
        let buf = [0x01u8, 0x00, 0x08, 0x08, 0xFF];
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn frames_enforces_per_frame_limits() {
        // Dimensions blow the default cap — the per-frame limit check in
        // decode_body must fire before any frame is allocated.
        let mut buf = Vec::new();
        write_header(32_000, 1, &mut buf);
        let err = parse_wbmp_frames(&buf).unwrap_err();
        assert!(matches!(err, WbmpError::LimitExceeded(_)), "{err:?}");
    }

    #[test]
    fn frames_with_unbounded_limits_decodes_large_main() {
        // 20000×1 (2500 bytes/frame) blows the default width cap but is
        // fine with unbounded limits; one main + one animated frame.
        let mut buf = Vec::new();
        write_header(20_000, 1, &mut buf);
        buf.extend_from_slice(&[0xAAu8; 2500]); // main
        buf.extend_from_slice(&[0x55u8; 2500]); // animated frame 1
        assert!(matches!(
            parse_wbmp_frames(&buf).unwrap_err(),
            WbmpError::LimitExceeded(_)
        ));
        let anim = parse_wbmp_frames_with_limits(&buf, &WbmpLimits::unbounded()).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.frames[0].data, vec![0xAAu8; 2500]);
        assert_eq!(anim.frames[1].data, vec![0x55u8; 2500]);
    }

    #[test]
    fn fuzz_frames_never_panic() {
        // Adversarial inputs to the animated-frame path must return a
        // Result, never panic / over-read / OOM.
        for a in 0u8..=255 {
            for b in 0u8..=255 {
                let _ = parse_wbmp_frames(&[a, b]);
            }
        }
        let mut seed: u64 = 0x0BAD_F00D_DEAD_C0DE;
        for _ in 0..4096 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = 1 + (seed as usize) % 40;
            let mut buf = vec![0u8; len];
            for byte in buf.iter_mut() {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = seed as u8;
            }
            let _ = parse_wbmp_frames(&buf);
        }
    }

    #[test]
    fn fuzz_padded_mbi_runs_never_panic() {
        // Adversarial header: Type=0, FixedHeader=0, then a long run
        // of continuation-bit-set bytes for both width and height. The
        // MBI cap should clamp without panicking or allocating.
        for run_len in 1..=12 {
            let mut buf = vec![0x00u8, 0x00];
            buf.extend(std::iter::repeat_n(0x80u8, run_len));
            buf.push(0x01); // closing byte
            buf.push(0x01); // height = 1
            buf.push(0x00); // 1 body byte
            let _ = parse_wbmp(&buf);
        }
    }
}
