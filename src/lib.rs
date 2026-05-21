//! Pure-Rust WBMP (WAP Bitmap) reader + writer.
//!
//! WBMP is the monochrome bitmap format defined by the WAP Forum for
//! early mobile-phone display use — see WAP-237 *Wireless Application
//! Environment Specification* (May 2001), §8 "Image Formats". The
//! spec defines a single-frame container with a four-field header and
//! a packed 1-bit-per-pixel pixel matrix; only "Type 0" (uncompressed
//! B/W bitmap) was ever standardised normatively or widely deployed,
//! so that's what this crate covers.
//!
//! ## Wire format (Type 0)
//!
//! ```text
//!   Type (MBI = 0)          1 byte
//!   FixedHeader             1 byte (always 0)
//!   Width  (MBI)            1..5 bytes
//!   Height (MBI)            1..5 bytes
//!   Pixel data              ceil(width / 8) * height bytes,
//!                           MSB-first, 1 = white, 0 = black,
//!                           rows zero-padded to the next byte.
//! ```
//!
//! `MBI` is the WAP "Multi-Byte Integer" — a variable-length
//! big-endian unsigned int with a continuation bit in the high bit of
//! every byte (see [`mbi`] for the codec helpers).
//!
//! ## Public API
//!
//! Standalone (always available):
//! * [`parse_wbmp`] — bytes → [`WbmpImage`].
//! * [`encode_wbmp`] — `(width, height, packed_bits)` → bytes.
//! * [`encode_wbmp_from_threshold`] — `(width, height, gray, threshold)` → bytes.
//!
//! Registry-gated (default-on `registry` feature, pulls
//! `oxideav-core`):
//! * [`registry::register`] / [`registry::register_codecs`] /
//!   [`registry::register_containers`] — wires up the codec +
//!   container into `oxideav-core` registries.
//!
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` / `Encoder` trait surface plus
//! a [`registry::register`] entry point. Disable the feature
//! (`default-features = false`) for an `oxideav-core`-free build that
//! still exposes the standalone [`parse_wbmp`] / [`encode_wbmp`] /
//! [`encode_wbmp_from_threshold`] API and the crate-local
//! [`WbmpImage`] / [`WbmpError`] / [`WbmpPixelFormat`] types.
//!
//! ## Source provenance
//!
//! Implemented clean-room from the publicly published WAP Forum
//! specification (WAP-237-WAESpec, May 2001). No external library
//! source was consulted, paraphrased, or cross-checked at any stage.

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod header;
pub mod image;
pub mod limits;
pub mod mbi;

#[cfg(feature = "registry")]
pub mod container;
#[cfg(feature = "registry")]
pub mod registry;

/// Codec id for WBMP image frames.
pub const CODEC_ID_STR: &str = "wbmp";

pub use decoder::{parse_wbmp, parse_wbmp_with_limits};
pub use encoder::{encode_wbmp, encode_wbmp_from_threshold};
pub use error::{Result, WbmpError};
pub use header::{parse_header, write_header, Header};
pub use image::{WbmpImage, WbmpPixelFormat, WbmpPlane};
pub use limits::WbmpLimits;
pub use mbi::{mbi_u32_len, read_mbi_u32, write_mbi_u32, MAX_MBI_BYTES, MAX_U32_MBI_BYTES};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
