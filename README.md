# oxideav-wbmp

[![CI](https://github.com/OxideAV/oxideav-wbmp/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-wbmp/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-wbmp.svg)](https://crates.io/crates/oxideav-wbmp) [![docs.rs](https://docs.rs/oxideav-wbmp/badge.svg)](https://docs.rs/oxideav-wbmp) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust WBMP (WAP Bitmap) image codec and container for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Covers
WBMP **Type 0** (uncompressed monochrome bitmap) — the only widely-
deployed WBMP variant — in one self-contained crate: the general-form
header with extension fields, the packed 1-bit main image and the up
to 15 animated sub-images that may follow it. Spec source: the publicly
published WAP Forum specification *WAP-237 Wireless Application
Environment Defined Media Type Specification* (15 May 2001), §4.

## Standalone use

`oxideav-wbmp` follows the OxideAV image-crate contract
(`IMAGE_CRATE_API`): the same small root vocabulary every
`oxideav-<format>` image crate exposes, usable with
`default-features = false` and no `oxideav-core`, returning pixels as
plain `Vec<u8>`.

```toml
[dependencies]
oxideav-wbmp = { version = "0.0", default-features = false }
```

```rust
let bytes = std::fs::read("in.wbmp")?;
if oxideav_wbmp::probe(&bytes) {
    let info  = oxideav_wbmp::info(&bytes)?;        // header only: width, height, frames
    let img   = oxideav_wbmp::decode(&bytes)?;      // WbmpImage, native 1-bit MonoBlack
    let rgba: Vec<u8> = img.to_rgba8();             // 0 / 255 per channel, alpha 255
    let (w, h) = (img.width(), img.height());

    let opts = oxideav_wbmp::EncodeOptions::default().with_dither();
    let out: Vec<u8> = oxideav_wbmp::encode_rgba8(w, h, &rgba, &opts)?;
    std::fs::write("out.wbmp", out)?;
}
```

| Item | Signature |
|---|---|
| `probe` | `fn(&[u8]) -> bool` — plausible Type-0 header (`Type` 0, well-formed `FixHeaderField` / extension region, non-zero dimensions ≤ `PROBE_MAX_DIMENSION`, first row present); allocation-free |
| `info` | `fn(&[u8]) -> Result<ImageInfo, Error>` — `width`, `height`, `format`, `frames` (1..=16), `has_alpha`, `color`, `has_icc` / `has_exif` / `has_xmp`, plus `fix_header`, `ext_fields`, `data_offset` |
| `decode` / `decode_with` | `fn(&[u8][, &DecodeOptions]) -> Result<WbmpImage, Error>` — main image, native layout |
| `decode_rgb8` / `decode_rgba8` | `-> Result<RgbImage / RgbaImage, Error>` — `{ width, height, data }`, tightly packed, 3 / 4 bytes per pixel |
| `decode_all` / `decode_all_with` | `-> Result<Vec<Frame>, Error>` — `Frame { image, delay: None, index }`; the main image plus every animated sub-image |
| `decode_from` | `fn<R: Read>(R) -> Result<WbmpImage, Error>` |
| `encode` | `fn(&WbmpImage, &EncodeOptions) -> Result<Vec<u8>, Error>` — either polarity, written as the wire layout |
| `encode_rgb8` / `encode_rgba8` / `encode_gray8` | `fn(w, h, &[u8], &EncodeOptions)` — 8-bit input quantised to 1 bit per `EncodeOptions::quantize` |
| `encode_to` | `fn<W: Write>(&WbmpImage, &EncodeOptions, W) -> Result<(), Error>` |
| `encode_frames` | `fn(&[WbmpImage], &EncodeOptions) -> Result<Vec<u8>, Error>` — the inverse of `decode_all` |
| `WbmpImage` | `{ width, height, format: PixelFormat, planes: Vec<Plane>, color: ColorInfo, metadata: Metadata }` (no palette) with `new` / `packed` / `from_bits` / `from_gray8` / `from_rgb8` / `from_rgba8`, `width()` / `height()` / `format()` / `stride()`, `as_bytes()` / `into_raw()`, `to_gray8()` / `to_rgb8()` / `to_rgba8()`, `is_white(x, y)`, `into_format()` |
| `PixelFormat` | `= WbmpPixelFormat`: `MonoBlack` (native, 1 = white), `MonoWhite` (0 = white) — names and polarity mirror `oxideav_core::PixelFormat` |
| `Error` | `= WbmpError`: `InvalidData`, `Unsupported`, `LimitExceeded`, `Io` |

`to_rgb8` / `to_rgba8` expand every bit to `0` or `255` (alpha `255`)
whatever the polarity; no colour management is applied. Constructors
validate geometry and return `Result`, so an inconsistent image cannot
exist and the conversions are infallible.

The pre-contract names — `parse_wbmp`, `parse_wbmp_strict`,
`parse_wbmp_with_limits`, `parse_wbmp_as`, `parse_wbmp_ext`,
`parse_wbmp_frames`, `parse_header*`, `encode_wbmp`, `encode_wbmp_ext`,
`encode_wbmp_frames`, `encode_wbmp_from_threshold`,
`encode_wbmp_from_dither`, `WbmpLimits`, `WbmpPlane`, `WbmpAnimation`,
`WbmpImageExt`, `Header` / `HeaderExt` — remain for one release as
`#[deprecated]` thin wrappers over the same implementation.

## Framework use

With the default-on `registry` feature the crate plugs into the
`oxideav-core` registry:

```rust
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_wbmp::register(&mut ctx);                      // codec "wbmp" + the .wbmp container
let dec = oxideav_wbmp::make_decoder(&params)?;        // / make_encoder
let frame: oxideav_core::VideoFrame = img.into();      // From<WbmpImage>: one packed 1-bit plane
let back = oxideav_wbmp::WbmpImage::from_video_frame(&frame, &params)?;
```

The trait-side `Decoder` / `Encoder` are thin adapters over the
standalone functions (one implementation). The decoder emits the wire
polarity (`PixelFormat::MonoBlack`) unless `params.pixel_format` asks
for `MonoWhite`; the encoder accepts `MonoBlack` (verbatim), `MonoWhite`
(inverted, padding re-zeroed) and `Gray8` (quantised per the
`quantize` / `threshold` encoder options, discoverable through the
registry's options schema). The demuxer hands the whole file out as one
keyframe packet and advertises `MonoBlack`; the muxer writes the
encoder's packet through unchanged. `register_codecs` /
`register_containers` / `register_registries` are the per-registry
halves. WBMP has no colour signalling or palette, so no side-channel is
stamped on frames.

## Supported layouts

Decode — the wire layout → native `PixelFormat`:

| Wire | `PixelFormat` | Notes |
|---|---|---|
| Type 0, 1 bit/pixel, MSB-first, `1` = white | `MonoBlack` | stride `ceil(width / 8)`, rows byte-padded; `DecodeOptions::format = MonoWhite` flips every bit during the row copy (padding stays zero) |

Encode — `WbmpImage::format` → wire:

| `PixelFormat` | Notes |
|---|---|
| `MonoBlack` | written verbatim (a padded stride is repacked, padding bits zeroed) |
| `MonoWhite` | inverted to the wire polarity |

`encode_rgb8` / `encode_rgba8` / `encode_gray8` reduce continuous-tone
input to 1 bit because WBMP cannot carry it: RGB becomes Rec. 601 luma
(`(299 R + 587 G + 114 B + 500) / 1000`), alpha is **dropped** (WBMP has
no alpha mechanism), then `Quantize::Threshold(t)` (`>= t` → white,
default 128) or `Quantize::Dither` (Floyd–Steinberg, 7/16 3/16 5/16
1/16, decision at 128) packs the bits. `encode` of a 1-bit image never
quantises. There is no `WbmpImage` the format cannot represent, so
`Error::Unsupported` is reserved for geometry that overflows `usize`.

## Options

`DecodeOptions` (`Default` + `with_*`): `max_width`, `max_height`,
`max_pixels`, `max_bytes` (packed plane bytes per frame; default 1 GiB,
`None` lifts it) — all checked against the header before any
allocation (`Error::LimitExceeded`) — `strict` (default `false`) and
the WBMP extra `format` (`MonoBlack` default / `MonoWhite`).

| | lenient (default) | `strict` |
|---|---|---|
| `FixHeaderField` | honoured per §4.4.1: a set presence bit means the `ExtFields` region is parsed and skipped before `Width` (non-conformant Type-0 producer), surfaced by `info` | must be `0x00` (§4.5.1 "Extension headers MUST NOT be presented in this format") |
| MBIs | a bounded run of redundant leading `0x80` octets tolerated | shortest encoding required (§4.3.1) |
| Trailing bytes | ignored (a partial trailing frame is padding) | ignored |

Both modes reject a non-zero `Type` (`Unsupported`), a zero dimension
and a truncated main image (`InvalidData`).

`EncodeOptions` (`Default` + `with_*`): `quantize` (`with_threshold(t)`
/ `with_dither()`, default threshold 128), `ext_fields:
Option<ExtFields>` (write a general-form header; `None` = conformant
Type 0) and `strict` (validate a Type-11 region's §4.4.3 character
classes before writing). WBMP has no compression or quality axis.

## Metadata and colour

WBMP carries no ICC / Exif / XMP / gamma, so `WbmpImage::metadata` is
always empty and the encoder ignores whatever a caller sets.
`WbmpImage::color` is the documented default `ColorInfo::wbmp_default()`
= full range, identity matrix (0), primaries and transfer unspecified
(2): a 1-bit sample is one of the two extremes of an achromatic signal
("the two states of pixel off and on", WAP-237 §4). `decode(encode(img))
== img` for planes, colour and metadata in both polarities
(`tests/image_crate_api.rs`, including a randomised property test over
odd widths and 1..=16-frame animations).

## Limits

Every function returns `Error` on hostile input, never panics (fuzzed:
`probe` / `info` / `decode` / `decode_with` / `decode_all` /
`decode_rgb8` / `decode_rgba8` plus the encoder, extension-header and
MBI paths — see *Fuzzing*). `DecodeOptions` limits fire before
allocation; the `stride × height` product is overflow-checked; the
extension-header region is capped at `MAX_EXT_FIELD_BYTES` (4096
octets); MBIs are capped at `MAX_MBI_BYTES` (7) octets. `info` applies
no limit (it allocates nothing beyond the parsed extension fields).

## Wire format (Type 0)

```text
  Type  (MBI = 0)         1 byte
  FixHeaderField          1 byte (0x00 in a conformant Type-0 file)
  [ExtFields]             only when FixHeaderField bit 7 is set (§4.4.1)
  Width  (MBI)            1..5 bytes
  Height (MBI)            1..5 bytes
  Main image              ceil(width / 8) * height bytes,
                          MSB-first, 1 = white, 0 = black,
                          rows zero-padded to the next byte.
  Animated images         0..15 further planes of the same size (§4.5.1)
```

`MBI` (Multi-Byte Integer) is the WAP variable-length unsigned
integer: payload bits are 7-per-byte big-endian, the high bit of every
byte is the continuation flag (1 = more bytes, 0 = last). The MBI
codec lives in [`mbi`](src/mbi.rs) and round-trips every value in the
`u32` range; oversize sequences are rejected to avoid silent
truncation.

### Polarity

WAP-237 §4.5.1 fixes the wire polarity at "white=1, black=0". In the
`oxideav_core::PixelFormat` naming that is **`MonoBlack`** ("0 =
black"); `MonoWhite` ("0 = white") is the inverse. Before the
image-crate contract this crate tagged the wire bytes `MonoWhite`, so
frames reached the framework with inverted meaning; the variants now
carry the core polarity (see the CHANGELOG), the plane bytes are
unchanged.

## Extension headers (`ExtFields`)

The general WBMP header format (WAP-237 §4.4.1) is
`TypeField FixHeaderField [ExtFields] Width Height` — an optional
extension-header region may sit between the FixHeaderField and the
Width MBI. The FixHeaderField's high bit (Table 4-3) is the
"ExtFields follow" presence flag, and bits 6-5 select the extension
type. WBMP **Type 0** conformantly fixes the FixHeaderField at `0x00`
(§4.5.1: "Extension headers MUST NOT be presented in this format"), so
a real shipped WBMP never carries any — but the format is defined, and
[`ext`](src/ext.rs) parses it in full:

| Type | Layout (§4.4.1, §4.4.3) |
|------|-------------------------|
| 00   | Multi-byte reserved bitfield; bit 7 of each octet is a "more data follows" continuation flag, the rest reserved. |
| 01   | Single reserved octet. |
| 10   | Single reserved octet. |
| 11   | Sequence of `ParameterHeader ParameterIdentifier ParameterValue` pairs. The `ParameterHeader` octet is `concat-flag | 3-bit identifier-size (1-8) | 4-bit value-size (1-16)`; the identifier is a US-ASCII string, the value alphanumeric (Table 4-4). |

The lenient `decode` / `info` honour the presence flag, so a
non-conformant Type-0 file carrying extension headers decodes to the
real bitmap (rather than mis-reading the first ExtField octet as the
width MBI) and `ImageInfo::ext_fields` surfaces the parsed region;
`encode` with `EncodeOptions::ext_fields` is the inverse writer. At the
region level `parse_ext_fields` / `write_ext_fields` and their
`_strict` twins are the depth API: the §4.4.3 / §4.2 ABNF is normative
about the Type-11 character classes (`ParameterIdentifier = 1*8CHAR`,
US-ASCII `%x01-7F`; `ParameterValue = 1*16(ALPHA / DIGIT)`), the lax
parser stores the bytes verbatim, the strict parser / writer reject an
out-of-class byte as `Error::InvalidData`. `Parameter::new` is the
validating constructor (1..=7 identifier / 1..=15 value bytes — the
3-/4-bit size fields cannot encode 8 / 16); `identifier_str` /
`value_str` return the bytes as `&str`. The Type-00 / 01 / 10 regions
carry opaque reserved octets, so strict and lax agree there.

## Animated sub-images

WAP-237 §4.2 defines `Image-data = Main-image 0*15Animated-image`:
after the single header, the main image's `stride × height` bytes are
followed by 0..15 further packed bitmaps of the **same** dimensions —
no per-frame header, no timing ("It is User Agent dependent how those
animated images are processed", §4.5.1). `decode_all` returns every
frame in stream order (`Frame::index` 0 = main image, `delay` always
`None`), `info().frames` counts them from the buffer length without
reading a pixel, and `encode_frames` writes them back; a trailing run
shorter than one frame is ignorable padding. `max_bytes` bounds each
frame's plane; the §4.5.1 cap bounds the total at 16 planes.

## Fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) harness lives
in [`fuzz/`](fuzz/) with twelve libFuzzer targets, all crash-free under
sustained sweeps with bounded RSS (the allocation guards hold against
adversarial headers). Every target builds the crate with
`default-features = false`, so the harness exercises the framework-free
standalone path and never links `oxideav-core`.

* `decode` — arbitrary bytes through the whole contract read side
  (`probe`, `info`, `decode`, strict and `MonoWhite` `decode_with`,
  `decode_all`, `decode_rgb8`, `decode_rgba8`) with the cross-function
  invariants pinned: `probe ⇒ info`, `info` + complete body ⇒ `decode`,
  `decode_all().len() == info.frames`, strict ⊆ lenient, inverse
  polarity = bytewise inversion with zero padding, raw paths = 0 / 255
  expansion.
* `decode_ext` — arbitrary bytes through the lenient `decode` + `info`
  pair over a fuzz-controlled-length extension-header region followed
  by the body copy.
* `strict_decode` — strict `decode_with` over arbitrary bytes; strict ⊆
  lenient with identical pixels, strict-accepted streams have
  `FixHeaderField == 0x00`.
* `header_ext` / `header_ext_strict` — the general-form header walk
  (`info`, and the depth `parse_header_ext_strict`) plus a
  `write_ext_fields` → `parse_ext_fields` round trip of any decoded
  region, lax and strict.
* `roundtrip` / `polarity` — synthesise a canonical plane, `encode`,
  `decode` (both polarities), assert bit-exact survival and the
  documented inversion + padding mask.
* `threshold` / `dither` — synthesise an 8-bit grey plane, `encode_gray8`
  with each `Quantize` rule, decode, compare against a bit-by-bit
  reference (threshold) and the saturated-input agreement (dither).
* `frames` — `decode_all` over arbitrary bytes (frame count, geometry,
  `info` agreement, frame 0 == `decode`) and a 1..=16-frame
  `encode_frames` → `decode_all` round trip.
* `encode_ext` — `encode` with every `ExtFields` variant, lax and strict,
  round-tripped through `decode` / `info`.
* `mbi` — the Multi-Byte Integer codec across the full `u32` value
  space (§4.3.1 invariants) and arbitrary reader input (strict ⊆ lax).

```sh
cargo +nightly fuzz run decode     # or any of the targets above
```

## Benchmarks

A Criterion suite in [`benches/`](benches/) covers the hot paths
end-to-end (`decode`, `encode`, full `roundtrip`, and the multi-frame
`frames` path) at representative sizes: 8×8 (per-call overhead), 96×64
(WAP-era handset), 320×240 (QVGA, 2-byte width MBI), 159×33 (odd-width
padding-bit boundary), 1024×1024 and 2048×2048. The `encode` bench also
exercises `encode_gray8` with the threshold and dither rules on a
320×240 grayscale fixture. Each scenario synthesises its fixture
in-process from a deterministic xorshift32 source (no fixture files on
disk). Run with:

```sh
cargo bench -p oxideav-wbmp --bench decode
cargo bench -p oxideav-wbmp --bench encode
cargo bench -p oxideav-wbmp --bench roundtrip
cargo bench -p oxideav-wbmp --bench frames
```

Indicative numbers on an Apple M1 Pro (release, single core): decode
tops out around 71 GiB/s on the 2048×2048 fixture (memory-copy bound),
encode at 60 GiB/s on the 1024×1024 fixture, end-to-end roundtrip at
22 GiB/s on 1024×1024, and the threshold quantiser at ~10 GiB/s on the
320×240 Gray8 fixture. The dither path is dominated by the
inherently-sequential Floyd–Steinberg residual diffusion, so its
headline throughput stays an order of magnitude below the threshold
path.

## Not supported

* WBMP Type values other than `0`. Later WAP releases reserved Type 1+
  for greyscale / colour bitmaps but never published a normative
  encoding, and no public devices shipped non-Type-0 content. Other
  Type values raise `Error::Unsupported`.
* **Animation timing.** The animated sub-image *frames* are decoded
  (see [Animated sub-images](#animated-sub-images)), but WAP-237 defines
  no normative animation timing parameters, so `Frame::delay` is always
  `None` and the registry decoder emits the main image only.

## License

MIT. See `LICENSE`.
