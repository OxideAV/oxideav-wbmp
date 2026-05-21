//! `oxideav-core` integration layer for `oxideav-wbmp`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-wbmp` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] / [`register_codecs`] / [`register_containers`] — the
//!   `CodecRegistry` / `ContainerRegistry` entry points the umbrella
//!   `oxideav` crate calls during framework initialisation.
//! * The `From<WbmpError> for oxideav_core::Error` conversion that lets
//!   the trait-side `Decoder` / `Encoder` impls (in `decoder.rs` /
//!   `encoder.rs`) bubble bitstream errors up through the framework
//!   error type.

use oxideav_core::ContainerRegistry;
use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};
use oxideav_core::{CodecInfo, CodecRegistry};

use crate::container;
use crate::error::WbmpError;

/// Convert a [`WbmpError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free encode/decode functions.
impl From<WbmpError> for oxideav_core::Error {
    fn from(e: WbmpError) -> Self {
        match e {
            WbmpError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            WbmpError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            WbmpError::LimitExceeded(s) => oxideav_core::Error::ResourceExhausted(s),
        }
    }
}

/// Register the WBMP codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("wbmp_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // WBMP is monochrome; the encoder accepts MonoWhite verbatim,
        // MonoBlack via a polarity flip, and Gray8 via threshold-128
        // convenience.
        .with_pixel_formats(vec![
            PixelFormat::MonoWhite,
            PixelFormat::MonoBlack,
            PixelFormat::Gray8,
        ]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::decoder::make_decoder)
            .encoder(crate::encoder::make_encoder),
    );
}

/// Register the WBMP container demuxer + muxer + extension + probe
/// into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Combined registration for callers that just want everything wired up
/// in one call.
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}
