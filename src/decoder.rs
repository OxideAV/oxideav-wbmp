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
use crate::header::parse_header;
use crate::image::{WbmpImage, WbmpPixelFormat, WbmpPlane};

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. One packet per whole
/// WBMP file; one frame per packet.
#[cfg(feature = "registry")]
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(WbmpDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct WbmpDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Decoder for WbmpDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_wbmp(&packet.data)?;
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

/// Decode a complete WBMP file (Type 0 only) into a [`WbmpImage`].
///
/// Returns:
/// * [`WbmpError::Unsupported`] if the Type field is non-zero (no
///   other type is defined by WAP-237 normatively or widely
///   deployed).
/// * [`WbmpError::InvalidData`] for truncated headers, MBI overflow,
///   or pixel-data shorter than what `width × height` requires.
pub fn parse_wbmp(input: &[u8]) -> Result<WbmpImage> {
    let header = parse_header(input)?;
    let stride = WbmpImage::row_stride(header.width);
    let expected = stride
        .checked_mul(header.height as usize)
        .ok_or_else(|| WbmpError::invalid("WBMP: width × height overflows usize"))?;

    let body = &input[header.data_offset..];
    if body.len() < expected {
        return Err(WbmpError::invalid(format!(
            "WBMP: pixel data truncated (need {expected} bytes, got {})",
            body.len()
        )));
    }

    // Byte layout matches our plane format directly — copy verbatim.
    // We allow trailing bytes past `expected` (some encoders pad to
    // even byte boundaries); we just drop them.
    let data = body[..expected].to_vec();

    Ok(WbmpImage {
        width: header.width,
        height: header.height,
        pixel_format: WbmpPixelFormat::MonoWhite,
        planes: vec![WbmpPlane { stride, data }],
        pts: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::write_header;

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
}
