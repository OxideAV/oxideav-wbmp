//! `oxideav-core` integration layer for `oxideav-wbmp`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-wbmp` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] — the unified `RuntimeContext` entry point the
//!   umbrella `oxideav` crate calls during framework initialisation
//!   (via the [`oxideav_core::register!`] macro). Internally calls
//!   [`register_codecs`] and [`register_containers`].
//! * [`register_codecs`] / [`register_containers`] — the per-registry
//!   halves; [`register_registries`] is the two-registry form.
//! * `From<WbmpImage> for VideoFrame` and [`WbmpImage::from_video_frame`]
//!   — the frame bridge (one packed 1-bit plane; WBMP signals no
//!   colour, so no side-channel is stamped), plus the 1:1
//!   [`WbmpPixelFormat`] ↔ `PixelFormat` mapping.
//! * The `From<WbmpError> for oxideav_core::Error` conversion that lets
//!   the trait-side `Decoder` / `Encoder` impls (in `decoder.rs` /
//!   `encoder.rs`) bubble errors up through the framework error type,
//!   and the `CodecOptionsStruct` impl for [`EncodeOptions`] (the
//!   `quantize` / `threshold` option schema).

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecOptionsStruct, CodecParameters, CodecRegistry,
    ContainerRegistry, OptionField, OptionKind, OptionValue, PixelFormat, RuntimeContext,
    VideoFrame, VideoPlane,
};

use crate::container;
use crate::error::WbmpError;
use crate::image::{WbmpImage, WbmpPixelFormat};
use crate::options::{EncodeOptions, Quantize};

/// Convert a [`WbmpError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free encode/decode functions.
impl From<WbmpError> for oxideav_core::Error {
    fn from(e: WbmpError) -> Self {
        match e {
            WbmpError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            WbmpError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            WbmpError::LimitExceeded(s) => oxideav_core::Error::ResourceExhausted(s),
            WbmpError::Io(e) => oxideav_core::Error::Io(e),
        }
    }
}

// ---- pixel formats --------------------------------------------------------

/// The 1:1 name (and polarity) mapping from [`WbmpPixelFormat`] to the
/// framework enum.
pub fn to_core_pixel_format(pf: WbmpPixelFormat) -> PixelFormat {
    match pf {
        WbmpPixelFormat::MonoBlack => PixelFormat::MonoBlack,
        WbmpPixelFormat::MonoWhite => PixelFormat::MonoWhite,
    }
}

/// Map a framework pixel format to [`WbmpPixelFormat`]; `Err` for the
/// layouts WBMP cannot carry.
pub fn from_core_pixel_format(pf: PixelFormat) -> crate::Result<WbmpPixelFormat> {
    match pf {
        PixelFormat::MonoBlack => Ok(WbmpPixelFormat::MonoBlack),
        PixelFormat::MonoWhite => Ok(WbmpPixelFormat::MonoWhite),
        other => Err(WbmpError::unsupported(format!(
            "WBMP: pixel format {other:?} not supported (MonoBlack / MonoWhite only)"
        ))),
    }
}

impl From<WbmpPixelFormat> for PixelFormat {
    fn from(pf: WbmpPixelFormat) -> Self {
        to_core_pixel_format(pf)
    }
}

impl TryFrom<PixelFormat> for WbmpPixelFormat {
    type Error = WbmpError;
    fn try_from(pf: PixelFormat) -> crate::Result<Self> {
        from_core_pixel_format(pf)
    }
}

// ---- frame bridge ---------------------------------------------------------

/// [`WbmpImage`] → `VideoFrame`, moving the plane out of the image: one
/// packed 1-bit plane and nothing else (WBMP carries no colour
/// signalling or palette, so no side-channel is attached).
pub(crate) fn image_into_video_frame(mut image: WbmpImage, pts: Option<i64>) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    VideoFrame {
        pts,
        planes: vec![VideoPlane { stride, data }],
    }
}

impl From<WbmpImage> for VideoFrame {
    /// The pixel plane, `pts` `None`.
    fn from(image: WbmpImage) -> Self {
        image_into_video_frame(image, None)
    }
}

impl From<&WbmpImage> for VideoFrame {
    fn from(image: &WbmpImage) -> Self {
        image_into_video_frame(image.clone(), None)
    }
}

impl WbmpImage {
    /// Rebuild an image from a framework frame and the stream
    /// parameters that describe it: `width`, `height` and
    /// `pixel_format` (`MonoBlack` / `MonoWhite`) are required; the
    /// frame's first image plane becomes the pixel plane (geometry
    /// validated by [`WbmpImage::new`]). Colour and metadata take
    /// WBMP's defaults (the format carries none).
    ///
    /// Errors with [`WbmpError::InvalidData`] for a missing parameter
    /// or plane / geometry mismatch and [`WbmpError::Unsupported`] for
    /// any other pixel format.
    pub fn from_video_frame(frame: &VideoFrame, params: &CodecParameters) -> crate::Result<Self> {
        let width = params
            .width
            .ok_or_else(|| WbmpError::invalid("WBMP: width missing in CodecParameters"))?;
        let height = params
            .height
            .ok_or_else(|| WbmpError::invalid("WBMP: height missing in CodecParameters"))?;
        let pix =
            from_core_pixel_format(params.pixel_format.ok_or_else(|| {
                WbmpError::invalid("WBMP: pixel_format missing in CodecParameters")
            })?)?;
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| WbmpError::invalid("WBMP: frame has no planes"))?;
        WbmpImage::packed(width, height, pix, plane.stride, plane.data.clone())
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for WbmpImage {
    type Error = WbmpError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> crate::Result<Self> {
        WbmpImage::from_video_frame(frame, params)
    }
}

// ---- CodecOptionsStruct (registry-only schema for EncodeOptions) ----------

/// The framework's options schema for the WBMP encoder — what makes the
/// quantisation knobs discoverable to `oxideav list`, validatable by
/// the pipeline's JSON-options checker, and parsed with uniform error
/// messages.
///
/// * `quantize`: `"threshold"` (the default) or `"dither"`
///   (Floyd–Steinberg) — how an 8-bit `Gray8` input frame is reduced to
///   1 bit; 1-bit input frames are never quantised.
/// * `threshold`: `0..=255` cutoff for `quantize = threshold` (samples
///   `>= threshold` are white; default 128). Setting it also selects
///   the threshold rule.
///
/// The extension-header knobs (`ext_fields` / `strict`) are not
/// exposed through the registry: they produce a Type-0-non-conformant
/// stream and exist for interop testing through the standalone API.
impl CodecOptionsStruct for EncodeOptions {
    const SCHEMA: &'static [OptionField] = &[
        OptionField {
            name: "quantize",
            kind: OptionKind::Enum(&["threshold", "dither"]),
            default: OptionValue::String(String::new()),
            help: "How 8-bit Gray8 input becomes 1-bit: \"threshold\" (default; \
                   see `threshold`) or \"dither\" (Floyd-Steinberg error diffusion). \
                   1-bit MonoBlack / MonoWhite input is written as-is.",
        },
        OptionField {
            name: "threshold",
            kind: OptionKind::U32,
            default: OptionValue::U32(128),
            help: "Grey cutoff 0..=255 for quantize=threshold: samples >= threshold \
                   are white (default 128). Also selects the threshold rule.",
        },
    ];

    fn apply(&mut self, key: &str, value: &OptionValue) -> oxideav_core::Result<()> {
        match key {
            "quantize" => {
                self.quantize = match value.as_str()? {
                    "threshold" => match self.quantize {
                        Quantize::Threshold(t) => Quantize::Threshold(t),
                        _ => Quantize::DEFAULT,
                    },
                    "dither" => Quantize::Dither,
                    // Unreachable in practice: the Enum schema already
                    // restricts the value set. Kept as a defensive arm.
                    other => {
                        return Err(oxideav_core::Error::invalid(format!(
                            "WBMP encoder: invalid quantize {other:?}"
                        )))
                    }
                };
                Ok(())
            }
            "threshold" => {
                let t = value.as_u32()?;
                if t > 255 {
                    return Err(oxideav_core::Error::invalid(format!(
                        "WBMP encoder: threshold {t} out of range 0..=255"
                    )));
                }
                self.quantize = Quantize::Threshold(t as u8);
                Ok(())
            }
            // Unreachable: parse_options rejects unknown keys against
            // SCHEMA before apply runs.
            other => Err(oxideav_core::Error::invalid(format!(
                "WBMP encoder: unknown option {other:?}"
            ))),
        }
    }
}

// ---- registration ---------------------------------------------------------

/// Register the WBMP codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("wbmp_sw")
        .with_intra_only(true)
        .with_lossless(true)
        // WBMP is monochrome: the encoder accepts MonoBlack (the wire
        // polarity) verbatim, MonoWhite via a polarity flip, and Gray8
        // via the `quantize` / `threshold` options.
        .with_pixel_formats(vec![
            PixelFormat::MonoBlack,
            PixelFormat::MonoWhite,
            PixelFormat::Gray8,
        ]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::decoder::make_decoder)
            .encoder(crate::encoder::make_encoder)
            .encoder_options::<EncodeOptions>(),
    );
}

/// Register the WBMP container demuxer + muxer + extension + probe
/// into the supplied [`ContainerRegistry`].
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
}

/// Two-registry registration (the pre-contract `register(codecs,
/// containers)` form; the fleet-wide [`register`] takes a
/// [`RuntimeContext`]).
pub fn register_registries(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

/// Unified entry point: install every codec and container provided by
/// `oxideav-wbmp` into a [`RuntimeContext`]. Also wired into
/// `oxideav_meta::register_all` via the [`oxideav_core::register!`]
/// macro below.
pub fn register(ctx: &mut RuntimeContext) {
    register_registries(&mut ctx.codecs, &mut ctx.containers);
}

/// Pre-contract name of [`register`].
#[deprecated(note = "use oxideav_wbmp::register(&mut RuntimeContext) (IMAGE_CRATE_API)")]
pub fn register_runtime(ctx: &mut RuntimeContext) {
    register(ctx);
}

oxideav_core::register!("wbmp", register);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::PixelFormat as WbmpPf;

    #[test]
    fn oxideav_entry_installs_codec_and_container() {
        let mut ctx = RuntimeContext::new();
        __oxideav_entry(&mut ctx);
        assert!(ctx.codecs.decoder_ids().next().is_some());
        assert_eq!(ctx.containers.container_for_extension("wbmp"), Some("wbmp"));
        let mut ctx2 = RuntimeContext::new();
        register(&mut ctx2);
        assert_eq!(
            ctx2.containers.container_for_extension("WBMP"),
            Some("wbmp")
        );
    }

    #[test]
    fn pixel_formats_map_one_to_one_by_name() {
        assert_eq!(
            to_core_pixel_format(WbmpPf::MonoBlack),
            PixelFormat::MonoBlack
        );
        assert_eq!(
            to_core_pixel_format(WbmpPf::MonoWhite),
            PixelFormat::MonoWhite
        );
        assert_eq!(
            WbmpPf::try_from(PixelFormat::MonoWhite).unwrap(),
            WbmpPf::MonoWhite
        );
        assert!(matches!(
            from_core_pixel_format(PixelFormat::Gray8),
            Err(WbmpError::Unsupported(_))
        ));
    }

    #[test]
    fn frame_bridge_round_trips_without_side_channels() {
        let img = WbmpImage::from_bits(11, 2, vec![0xAA, 0xA0, 0x55, 0x40]).unwrap();
        let frame: VideoFrame = img.clone().into();
        assert_eq!(frame.planes.len(), 1);
        assert_eq!(frame.image_planes().len(), 1);
        assert!(frame.color_signal().is_none());
        assert!(frame.palette().is_none());
        let mut params = CodecParameters::video(CodecId::new("wbmp"));
        params.width = Some(11);
        params.height = Some(2);
        params.pixel_format = Some(PixelFormat::MonoBlack);
        let back = WbmpImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(back, img);
        let back2 = WbmpImage::try_from((&frame, &params)).unwrap();
        assert_eq!(back2, img);
        // Polarity follows the parameters.
        params.pixel_format = Some(PixelFormat::MonoWhite);
        assert_eq!(
            WbmpImage::from_video_frame(&frame, &params)
                .unwrap()
                .format(),
            WbmpPf::MonoWhite
        );
        // Missing / wrong parameters are crate errors.
        params.pixel_format = Some(PixelFormat::Rgb24);
        assert!(matches!(
            WbmpImage::from_video_frame(&frame, &params),
            Err(WbmpError::Unsupported(_))
        ));
        params.pixel_format = None;
        assert!(matches!(
            WbmpImage::from_video_frame(&frame, &params),
            Err(WbmpError::InvalidData(_))
        ));
        params.pixel_format = Some(PixelFormat::MonoBlack);
        params.height = Some(3);
        assert!(matches!(
            WbmpImage::from_video_frame(&frame, &params),
            Err(WbmpError::InvalidData(_))
        ));
    }

    #[test]
    fn encoder_options_schema_is_discoverable_and_parses() {
        let mut reg = CodecRegistry::new();
        register_codecs(&mut reg);
        let schema = reg
            .encoder_options_schema(&CodecId::new(crate::CODEC_ID_STR))
            .expect("WBMP encoder should expose an options schema");
        assert!(schema.iter().any(|f| f.name == "quantize"));
        assert!(schema.iter().any(|f| f.name == "threshold"));

        use oxideav_core::{parse_options, CodecOptions};
        let parse = |pairs: &[(&str, &str)]| -> oxideav_core::Result<EncodeOptions> {
            let mut bag = CodecOptions::new();
            for (k, v) in pairs {
                bag.insert(*k, *v);
            }
            parse_options(&bag)
        };
        assert_eq!(
            parse(&[("quantize", "dither")]).unwrap().quantize,
            Quantize::Dither
        );
        assert_eq!(
            parse(&[("threshold", "200")]).unwrap().quantize,
            Quantize::Threshold(200)
        );
        assert_eq!(
            parse(&[("threshold", "7"), ("quantize", "threshold")])
                .unwrap()
                .quantize,
            Quantize::Threshold(7)
        );
        assert_eq!(parse(&[]).unwrap(), EncodeOptions::default());
        assert!(parse(&[("threshold", "256")]).is_err());
        assert!(parse(&[("quantize", "x")]).is_err());
        assert!(parse(&[("bogus", "1")]).is_err());
    }

    #[test]
    fn errors_map_to_core() {
        let e: oxideav_core::Error = WbmpError::limit_exceeded("x").into();
        assert!(matches!(e, oxideav_core::Error::ResourceExhausted(_)));
        let e: oxideav_core::Error = WbmpError::from(std::io::Error::other("x")).into();
        assert!(matches!(e, oxideav_core::Error::Io(_)));
    }
}
