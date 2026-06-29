//! WBMP container: one single-image file becomes one [`Packet`] on
//! stream `0`. Mirrors the same shape as `oxideav-bmp` /
//! `oxideav-pbm` (single-frame path) — WBMP has no animation or
//! multi-frame layout.
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
    ContainerRegistry, Demuxer, Muxer, ProbeData, ProbeScore, ReadSeek, WriteSeek,
    PROBE_SCORE_EXTENSION,
};

use crate::header::parse_header;
use crate::limits::WbmpLimits;

pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("wbmp", open_demuxer);
    reg.register_muxer("wbmp", open_muxer);
    reg.register_extension("wbmp", "wbmp");
    reg.register_probe("wbmp", probe);
}

/// WBMP has no magic number — Type 0 starts with a literal `0x00`
/// byte, which is far too common to claim a high-confidence content
/// probe. We therefore lean on the file extension when present and
/// only return a low score for content that *parses* as a valid Type-0
/// header. This avoids stealing probe wins from other formats whose
/// payload happens to start with a few null bytes.
///
/// The §4.1 precedence rule ("the actual data format has precedence over
/// the media type indicated in the `Content-Type` header") motivates a
/// content sniff that is more than a four-byte header parse: a buffer
/// that *parses* as a 1×1 header but is followed by megabytes of
/// unrelated data is almost certainly not a WBMP. So the content path
/// additionally requires that (a) the declared dimensions are within the
/// default [`WbmpLimits`] (a header claiming a 4-billion-pixel row is
/// noise, not a tiny real image), and (b) the buffer actually holds at
/// least the first full pixel row (`stride` bytes past the header) — a
/// coincidental null-byte run essentially never satisfies that, while a
/// genuine WBMP always does. Probe buffers are truncated previews, so we
/// only require the *first* row to be present, not the whole image.
pub fn probe(data: &ProbeData) -> ProbeScore {
    if matches!(data.ext, Some("wbmp")) {
        return PROBE_SCORE_EXTENSION;
    }
    // Content sniff: must look like a valid Type-0 header AND be at
    // least long enough to hold the smallest plausible image (1×1 →
    // 4-byte header + 1 body byte = 5 bytes).
    if data.buf.len() < 5 {
        return 0;
    }
    let Ok(header) = parse_header(data.buf) else {
        return 0;
    };
    // Reject headers whose declared dimensions exceed the default limits:
    // a plausible WBMP preview describes a real, bounded bitmap, whereas a
    // few coincidental continuation bytes can decode to an absurd MBI.
    let limits = WbmpLimits::default();
    if header.width > limits.max_width || header.height > limits.max_height {
        return 0;
    }
    // Require the first pixel row to be present in the (possibly
    // truncated) probe buffer. stride = ceil(width / 8); the body starts
    // at header.data_offset.
    let stride = (header.width as usize).div_ceil(8);
    let first_row_end = header.data_offset.saturating_add(stride);
    if data.buf.len() < first_row_end {
        return 0;
    }
    // Half of PROBE_SCORE_EXTENSION (intentionally conservative).
    PROBE_SCORE_EXTENSION / 2
}

pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    let header = parse_header(&buf)?;
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(header.width);
    params.height = Some(header.height);
    params.pixel_format = Some(PixelFormat::MonoWhite);
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
