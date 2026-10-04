//! Pure-Rust WBMP (WAP Bitmap) reader + writer.
//!
//! WBMP is the monochrome bitmap format defined by the WAP Forum for
//! early mobile-phone display use — see WAP-237 *Wireless Application
//! Environment Defined Media Type Specification* (May 2001), §4. The
//! spec defines a container with a general-form header and a packed
//! 1-bit-per-pixel pixel matrix, optionally followed by up to 15
//! same-size animated sub-images; only "Type 0" (uncompressed B/W
//! bitmap) was ever standardised normatively or widely deployed, so
//! that's what this crate covers.
//!
//! ## Wire format (Type 0)
//!
//! ```text
//!   Type (MBI = 0)          1 byte
//!   FixHeaderField          1 byte (0x00 in a conformant Type-0 file;
//!                           bit 7 set = ExtFields follow, §4.4.1)
//!   [ExtFields]             bitfield chain / reserved octet / parameter pairs
//!   Width  (MBI)            1..5 bytes
//!   Height (MBI)            1..5 bytes
//!   Main image              ceil(width / 8) * height bytes,
//!                           MSB-first, 1 = white, 0 = black,
//!                           rows zero-padded to the next byte.
//!   Animated images         0..15 further planes of the same size
//! ```
//!
//! `MBI` is the WAP "Multi-Byte Integer" — a variable-length
//! big-endian unsigned int with a continuation bit in the high bit of
//! every byte (see [`mbi`] for the codec helpers).
//!
//! ## Standalone use (`IMAGE_CRATE_API`)
//!
//! The crate follows the OxideAV image-crate contract: the same small
//! root vocabulary every `oxideav-<format>` image crate exposes, usable
//! with `default-features = false` and no `oxideav-core`.
//!
//! ```
//! # fn main() -> Result<(), oxideav_wbmp::Error> {
//! # let bytes = oxideav_wbmp::encode_gray8(9, 2, &[0, 255, 0, 255, 0, 255, 0, 255, 0,
//! #     255, 0, 255, 0, 255, 0, 255, 0, 255], &oxideav_wbmp::EncodeOptions::default())?;
//! if oxideav_wbmp::probe(&bytes) {
//!     let info = oxideav_wbmp::info(&bytes)?;      // header only: width, height, frames
//!     let img  = oxideav_wbmp::decode(&bytes)?;    // WbmpImage, native 1-bit MonoBlack
//!     let rgba: Vec<u8> = img.to_rgba8();          // 0 / 255 per channel, alpha 255
//!     assert_eq!(rgba.len(), 4 * info.width as usize * info.height as usize);
//!
//!     let opts = oxideav_wbmp::EncodeOptions::default().with_dither();
//!     let out  = oxideav_wbmp::encode_rgba8(img.width(), img.height(), &rgba, &opts)?;
//!     assert_eq!(oxideav_wbmp::decode(&out)?, img);
//! }
//! # Ok(()) }
//! ```
//!
//! * [`probe`] / [`info`] / [`decode`] / [`decode_with`] /
//!   [`decode_rgb8`] / [`decode_rgba8`] / [`decode_all`] /
//!   [`decode_all_with`] / [`decode_from`] — the read side.
//! * [`encode`] / [`encode_rgb8`] / [`encode_rgba8`] / [`encode_gray8`]
//!   / [`encode_to`] / [`encode_frames`] — the write side.
//! * [`WbmpImage`], [`RgbImage`], [`RgbaImage`], [`ImageInfo`],
//!   [`Frame`], [`DecodeOptions`], [`EncodeOptions`], [`Quantize`],
//!   [`PixelFormat`], [`Error`] — the types.
//!
//! The pre-contract `parse_wbmp*` / `encode_wbmp*` / `parse_header*`
//! family and `WbmpLimits` remain for one release as `#[deprecated]`
//! wrappers.
//!
//! ## Framework use
//!
//! The default-on `registry` feature pulls in `oxideav-core` and
//! exposes `register(&mut RuntimeContext)`, `make_decoder` /
//! `make_encoder`, the `From<WbmpImage> for VideoFrame` /
//! `WbmpImage::from_video_frame` bridge and the container
//! (demuxer, muxer, `.wbmp` extension, content probe). The trait-side
//! `Decoder` / `Encoder` are thin adapters over the standalone
//! functions.
//!
//! ## Source provenance
//!
//! Implemented clean-room from the publicly published WAP Forum
//! specification (WAP-237-WAEMT-20010515-a).

mod api;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod ext;
#[doc(hidden)]
pub mod header;
pub mod image;
#[doc(hidden)]
pub mod limits;
pub mod mbi;
pub mod options;
mod quantize;

#[cfg(feature = "registry")]
pub mod container;
#[cfg(feature = "registry")]
pub mod registry;

/// Codec id for WBMP image frames.
pub const CODEC_ID_STR: &str = "wbmp";

// ---- the contract vocabulary ----------------------------------------------
pub use api::{
    decode, decode_all, decode_all_with, decode_from, decode_rgb8, decode_rgba8, decode_with,
    encode, encode_all, encode_gray8, encode_rgb8, encode_rgba8, encode_to, info, probe,
    PROBE_MAX_DIMENSION,
};
pub use decoder::MAX_ANIMATED_IMAGES;
pub use encoder::encode_frames;
pub use error::{Error, Result, WbmpError};
pub use image::{
    ColorInfo, ColorRange, Frame, ImageInfo, Metadata, PixelFormat, Plane, RgbImage, RgbaImage,
    WbmpImage, WbmpPixelFormat,
};
pub use options::{DecodeOptions, EncodeOptions, Quantize};

// ---- format depth (extension headers, MBIs) -------------------------------
pub use ext::{
    parse_ext_fields, parse_ext_fields_strict, write_ext_fields, write_ext_fields_strict,
    ExtFieldType, ExtFields, FixHeaderField, Parameter, MAX_EXT_FIELD_BYTES,
};
#[doc(hidden)]
pub use header::{write_header, write_header_ext};
#[doc(hidden)]
pub use image::PlaneLayout;
pub use mbi::{
    mbi_u32_len, read_mbi_u32, read_mbi_u32_strict, write_mbi_u32, MAX_MBI_BYTES, MAX_U32_MBI_BYTES,
};

// ---- deprecated pre-contract entry points (one release) --------------------
#[allow(deprecated)]
pub use decoder::{
    parse_wbmp, parse_wbmp_as, parse_wbmp_as_with_limits, parse_wbmp_ext,
    parse_wbmp_ext_with_limits, parse_wbmp_frames, parse_wbmp_frames_with_limits,
    parse_wbmp_strict, parse_wbmp_strict_with_limits, parse_wbmp_with_limits, WbmpAnimation,
    WbmpImageExt,
};
#[allow(deprecated)]
pub use encoder::{
    encode_wbmp, encode_wbmp_ext, encode_wbmp_frames, encode_wbmp_from_dither,
    encode_wbmp_from_threshold,
};
#[allow(deprecated)]
pub use header::{
    parse_header, parse_header_ext, parse_header_ext_strict, parse_header_strict, Header, HeaderExt,
};
#[allow(deprecated)]
pub use image::WbmpPlane;
#[allow(deprecated)]
pub use limits::WbmpLimits;

#[cfg(feature = "registry")]
pub use decoder::make_decoder;
#[cfg(feature = "registry")]
pub use encoder::make_encoder;
#[cfg(feature = "registry")]
#[allow(deprecated)]
pub use registry::register_runtime;
#[cfg(feature = "registry")]
pub use registry::{
    __oxideav_entry, from_core_pixel_format, register, register_codecs, register_containers,
    register_registries, to_core_pixel_format,
};
