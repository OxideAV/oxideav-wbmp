//! WBMP container: one file becomes one [`Packet`] on stream `0`
//! (the decoder returns its main image; the WAP-237 §4.5.1 animated
//! sub-images are reachable through the standalone
//! [`crate::decode_all`]). Mirrors the same shape as `oxideav-bmp` /
//! `oxideav-pbm` (single-frame path).
//!
//! Lives behind the `registry` feature: the container types are all
//! defined by `oxideav-core`, so a standalone build (no framework dep)
//! has nothing meaningful to expose here.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, Error, MediaType, Packet, PixelFormat, Result,
    StreamInfo, TimeBase,
};
use oxideav_core::{
    ContainerRegistry, Demuxer, Muxer, PROBE_SCORE_EXTENSION, ProbeData, ProbeScore, ReadSeek,
    WriteSeek,
};

use crate::api::Sniff;
use crate::header::parse_header_ext_inner;

/// Register the demuxer, muxer, `.wbmp` extension hint and content
/// probe into `reg`.
pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("wbmp", open_demuxer);
    reg.register_muxer("wbmp", open_muxer);
    reg.register_extension("wbmp", "wbmp");
    reg.register_probe("wbmp", probe);
}

/// Content score for a buffer that is exactly a whole WBMP file
/// ([`crate::probe`]); the `.wbmp` extension lifts it to `75`.
pub const PROBE_SCORE_EXACT: ProbeScore = 50;

/// WBMP has no magic number — a Type-0 file starts `00 00` followed by
/// two MBI dimensions, a prefix that countless other byte streams share
/// (an MPEG program-stream pack header `00 00 01 BA` parses as a 1×7457
/// Type-0 header). The probe therefore demands the strict WAP-237
/// §4.5.1 Type-0 header (no extension headers, reserved bits zero,
/// shortest-form non-zero dimensions at most
/// [`crate::PROBE_MAX_DIMENSION`]), zero padding bits in every row
/// present, and:
///
/// * a buffer that is **exactly** the header plus `1..=16` whole images
///   (main + 0..15 animated, §4.2): [`PROBE_SCORE_EXACT`], or `75` with
///   the `.wbmp` extension hint;
/// * a buffer that ends inside the image data (the input is larger than
///   the probe window): [`PROBE_SCORE_EXTENSION`] only when the hint is
///   `.wbmp` — the length cannot be checked, so content alone never
///   claims such a buffer;
/// * anything else: `0`, extension hint or not (the registry's own
///   extension fallback still applies when no probe scores).
pub fn probe(data: &ProbeData) -> ProbeScore {
    let ext = matches!(data.ext, Some("wbmp"));
    match crate::api::sniff(data.buf) {
        Some(Sniff::Exact) if ext => PROBE_SCORE_EXACT + PROBE_SCORE_EXTENSION,
        Some(Sniff::Exact) => PROBE_SCORE_EXACT,
        Some(Sniff::Truncated) if ext => PROBE_SCORE_EXTENSION,
        _ => 0,
    }
}

/// Demuxer factory: reads the whole input, validates the header and
/// hands the file out as one keyframe packet.
pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    // Lenient general-form header parse (extension headers honoured),
    // the same one `crate::info` / `crate::decode` use.
    let header = parse_header_ext_inner(&buf, false)?;
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(header.width);
    params.height = Some(header.height);
    // The wire polarity (WAP-237 §4.5.1: 1 = white) is the core
    // `MonoBlack` layout ("0 = black").
    params.pixel_format = Some(PixelFormat::MonoBlack);
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: TimeBase::new(1, 1),
        start_time: Some(0),
        duration: None,
    };
    Ok(Box::new(WbmpDemuxer {
        streams: vec![stream],
        data: Some(buf),
    }))
}

struct WbmpDemuxer {
    streams: Vec<StreamInfo>,
    data: Option<Vec<u8>>,
}

impl Demuxer for WbmpDemuxer {
    fn format_name(&self) -> &str {
        "wbmp"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.data.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => Err(Error::Eof),
        }
    }
}

/// Muxer factory: writes the single encoder packet through unchanged.
pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid(
            "WBMP muxer: expected exactly one video stream",
        ));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("WBMP muxer: stream must be video"));
    }
    Ok(Box::new(WbmpMuxer { output }))
}

struct WbmpMuxer {
    output: Box<dyn WriteSeek>,
}

impl Muxer for WbmpMuxer {
    fn format_name(&self) -> &str {
        "wbmp"
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        // The encoder produces a complete WBMP file in a single
        // packet — write it through unchanged.
        self.output.write_all(&packet.data)?;
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        Ok(())
    }
}
