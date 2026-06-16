# oxideav-wbmp

Pure-Rust WBMP (WAP Bitmap) image codec and container for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Covers
WBMP **Type 0** (uncompressed monochrome bitmap) — the only widely-
deployed WBMP variant — in one self-contained crate. Spec source: the
publicly published WAP Forum specification *WAP-237 Wireless
Application Environment Specification* (May 2001), §8 "Image
Formats".

## Wire format (Type 0)

```text
  Type  (MBI = 0)         1 byte
  FixedHeader             1 byte (always 0)
  Width  (MBI)            1..5 bytes
  Height (MBI)            1..5 bytes
  Pixel data              ceil(width / 8) * height bytes,
                          MSB-first, 1 = white, 0 = black,
                          rows zero-padded to the next byte.
```

`MBI` (Multi-Byte Integer) is the WAP variable-length unsigned
integer: payload bits are 7-per-byte big-endian, the high bit of every
byte is the continuation flag (1 = more bytes, 0 = last). The MBI
codec lives in [`mbi`](src/mbi.rs) and round-trips every value in the
`u32` range; oversize sequences are rejected to avoid silent
truncation.

## Decode

| Type | Channels | Bit depth | `PixelFormat` out |
|------|----------|-----------|-------------------|
| 0    | 1 (1-bit) | 1        | `MonoWhite` (verbatim) or `MonoBlack` (caller-selected polarity) |

Other Type values raise `WbmpError::Unsupported`. None ever shipped
in public WAP profiles.

## Lax vs strict header conformance

`parse_wbmp` (lax) accepts any value for the one-byte `FixedHeader`
field — the spec text leaves the byte unused in Type 0 but mandatory
in the wire format, and treating it as opaque keeps the decoder
forward-compatible with hypothetical Type-0 extensions. Callers that
need wire-format conformance instead — reject anything whose
`FixedHeader` is not the spec-mandated `0x00` — reach for
`parse_wbmp_strict` (and `parse_wbmp_strict_with_limits` for the
explicit-limits variant). The strict path errors out as
`WbmpError::InvalidData` with a message naming the offending byte;
all other checks (Type-field, zero-dimension, MBI bounds, limits,
truncation) are identical to the lax path. The header-level entry
points `parse_header` / `parse_header_strict` expose the same split
for callers that want to inspect the four-field header without
touching the pixel plane.

The strict path additionally enforces the §4.3.1 **shortest-encoding**
requirement on every multi-byte integer: the spec states the encoded
value "MUST NOT start with an octet with the value `0x80`", i.e. a
redundant leading continuation octet that carries no payload bits. The
lax path tolerates a bounded amount of such leading-`0x80` padding (a
few real files pad despite the MUST NOT); the strict path rejects it
as `WbmpError::InvalidData` on the Type, Width or Height MBI. An
*interior* `0x80` group is still accepted in both paths — it is a
legitimate all-zero 7-bit group with more bytes following (e.g.
`0x4000` minimally encodes as `0x81 0x80 0x00`); only a *leading*
`0x80` is forbidden. The standalone reader pair is `read_mbi_u32`
(lax) / `read_mbi_u32_strict`.

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

`parse_ext_fields` decodes a region given a `FixHeaderField`;
`write_ext_fields` is the inverse serializer. The header-level
[`parse_header_ext`] returns a `HeaderExt` (width / height /
data_offset + the decoded FixHeaderField + `Option<ExtFields>`) that
honours the presence flag, so the decoder lands on the real
Width/Height rather than mis-reading the first ExtField octet as the
width MBI when a non-conformant Type-0 file carries extension headers.
A `MAX_EXT_FIELD_BYTES` (4096) ceiling bounds pathological
all-continuation chains. The plain `parse_header` / `parse_wbmp` paths
are unchanged — they treat the FixHeaderField byte as opaque (the
forward-compat lax behaviour documented above), so this is a purely
additive entry point.

For a full decode of an extension-header-bearing stream into pixels,
`parse_wbmp_ext` (and `parse_wbmp_ext_with_limits`) route through
`parse_header_ext` and then copy the main image data, returning a
`WbmpImageExt { image, ext_fields }`. On a conformant Type-0 file the
`image` is byte-identical to `parse_wbmp`'s output and `ext_fields` is
`None`; on a non-conformant file carrying extension headers it decodes
the real bitmap (rather than failing on the first ExtField octet
mistaken for the width MBI) and surfaces the parsed pairs/bitfield.

`parse_wbmp` (and `parse_wbmp_with_limits`) emit the on-disk
polarity unchanged — `WbmpPixelFormat::MonoWhite`, where bit `1` is
white. Callers that want the inverted polarity for downstream
consumers reach for `parse_wbmp_as` (or `parse_wbmp_as_with_limits`):

```rust
use oxideav_wbmp::{parse_wbmp_as, WbmpPixelFormat};
let img = parse_wbmp_as(&bytes, WbmpPixelFormat::MonoBlack)?;
// img.planes[0].data: every payload bit inverted, every row's
// trailing padding bits re-zeroed so they stay distinguishable
// from real `1`-bit "black" pixels on inspection.
```

The polarity flip happens in-place during the decode-time row copy
— no extra allocation versus the verbatim path. Under the
default-on `registry` feature, setting
`params.pixel_format = Some(PixelFormat::MonoBlack)` on the
`CodecParameters` handed to the framework decoder selects the same
behaviour through the `Decoder` trait.

## Animated sub-images

WAP-237 §4.2 defines the full image-data grammar as
`Image-data = Main-image 0*15Animated-image`, and §4.5.1 states the
stream "can have at most 15 animated images following the main image."
Each animated sub-image is a bitmap "formed according to image data
structure specified by the TypeField" — for Type 0 that is an
identically-dimensioned packed 1-bit plane carried with **no per-frame
header**: the single header `Width`/`Height` govern every frame, so each
sub-image occupies exactly `stride * height` bytes immediately after the
previous one.

`parse_wbmp_frames` (and `parse_wbmp_frames_with_limits`) decode the main
image plus any trailing sub-images into a `WbmpAnimation`:

```rust
use oxideav_wbmp::parse_wbmp_frames;
let anim = parse_wbmp_frames(&bytes)?;
// anim.frames[0] is the main image; anim.frames[1..] the animated
// sub-images in stream order.
if anim.is_animated() {
    println!("{} animated sub-images", anim.animated_count());
}
let main = anim.main_image(); // frame 0 as a standalone WbmpImage
```

The decoder greedily consumes each following `stride * height` chunk as a
sub-image until either fewer than one full frame of bytes remain (the
trailing run is then treated as ignorable padding, matching the
single-frame path's tolerance of trailing bytes) or the §4.5.1 cap of
`MAX_ANIMATED_IMAGES` (15) animated frames is reached. The per-frame
`WbmpLimits` dimension / pixel-byte checks reuse the single-frame guards,
so an over-sized header is rejected before any frame is allocated. On a
non-animated WBMP the returned `frames` holds exactly one plane,
byte-identical to `parse_wbmp`'s output. WAP-237 defines no animation
timing parameters ("It is User Agent dependent how those animated images
are processed"), so the crate surfaces the raw frame planes and leaves
presentation timing to the caller. The single-frame `parse_wbmp` entry
point is unchanged.

`encode_wbmp_frames(width, height, &[main, anim…])` is the inverse: it
writes the single four-field header followed by the main image plane and
0..15 same-dimension animated sub-image planes back-to-back (no per-frame
header, matching the §4.2 BNF). Every frame must be exactly
`ceil(width / 8) * height` bytes — the same packed-plane shape
`encode_wbmp` accepts. The §4.5.1 cap (`1 + MAX_ANIMATED_IMAGES == 16`
frames total), zero dimensions, an empty frame list, and any wrong-sized
frame all raise `WbmpError::InvalidData`. A single-element call is
byte-for-byte identical to `encode_wbmp` with the same plane, so a
non-animated caller can use either entry point, and an
`encode_wbmp_frames` → `parse_wbmp_frames` round trip recovers every
plane in stream order.

## Encode

[`encode_wbmp`] takes an already-packed mono plane (1 bit per pixel,
MSB-first, 1 = white, rows padded to a byte boundary) and prepends a
Type-0 header. [`encode_wbmp_from_threshold`] thresholds an 8-bit
grayscale buffer (one byte per pixel, no row padding) at the supplied
cut-off and produces a complete WBMP file in one call.
[`encode_wbmp_from_dither`] takes the same 8-bit grayscale buffer
and runs a Floyd–Steinberg error-diffusion quantiser before packing,
so a smoothly-shaded photographic input lands as a stippled rendering
rather than collapsing every mid-tone to a flat region. The dither
helper uses an i16 row-accumulator (`O(width)` extra space) and the
classic 7/16, 3/16, 5/16, 1/16 forward-neighbour distribution;
saturated black/white pixels diffuse zero residual, so flat-mono
input agrees byte-for-byte with `encode_wbmp_from_threshold(.., 128)`.
Reference: R. W. Floyd and L. Steinberg, "An adaptive algorithm for
spatial greyscale", *Proc. SID* 17/2 (1976), pp. 75–77.

When the `registry` feature is on, the framework `Encoder` trait
accepts `MonoWhite` (verbatim), `MonoBlack` (polarity-flipped, with
padding bits re-zeroed) and `Gray8` (thresholded at 128 by default).

## Standalone vs registry-integrated

The crate's default `registry` Cargo feature pulls in `oxideav-core`
and exposes the framework `Decoder` / `Encoder` trait surface plus a
`registry::register` entry point. Disable the feature
(`default-features = false`) for an `oxideav-core`-free build that
still exposes the standalone `parse_wbmp` / `encode_wbmp` /
`encode_wbmp_from_threshold` API and the crate-local `WbmpImage` /
`WbmpError` / `WbmpPixelFormat` types.

## Registration

```rust
let mut codecs = oxideav_core::CodecRegistry::new();
let mut containers = oxideav_core::ContainerRegistry::new();
oxideav_wbmp::register(&mut codecs, &mut containers);
```

## Resource limits

`parse_wbmp` enforces a default [`WbmpLimits`] (max width 16 384, max
height 16 384, max packed pixel-data 8 MiB) so an attacker-crafted
header carrying `u32::MAX × u32::MAX` dimensions can't make the decoder
allocate hundreds of gigabytes. Headers exceeding any limit return
`WbmpError::LimitExceeded` (mapped to
`oxideav_core::Error::ResourceExhausted` under `registry`) *before*
the decoder touches its allocator.

Callers that need to admit larger images:

```rust
use oxideav_wbmp::{parse_wbmp_with_limits, WbmpLimits};
let img = parse_wbmp_with_limits(&buf, &WbmpLimits::unbounded())?;
```

The MBI decoder is similarly bounded: `MAX_MBI_BYTES = 7` caps the
length of any single MBI sequence (5 bytes for the minimal `u32`
encoding + a 2-byte allowance for leading `0x80` padding the spec
text doesn't outlaw). Pathological continuation-byte runs error in
O(1) rather than chasing the input.

## Fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) harness lives
in [`fuzz/`](fuzz/) with seven libFuzzer targets, all crash-free under
sustained sweeps with bounded RSS (the allocation guards hold against
adversarial headers):

* `decode` — feeds arbitrary bytes to `parse_wbmp`; the decoder must
  return a `Result` and never panic / abort / OOM. The classic overflow
  spots are the multi-byte width/height MBI parse and the
  `stride * height` pixel-buffer allocation; both are guarded
  (`checked_mul`, the `MAX_MBI_BYTES` ceiling, the default `WbmpLimits`)
  and this target keeps them honest.
* `roundtrip` — synthesises a valid Type-0 file from fuzz-controlled
  small dimensions + packed bits, decodes it, and asserts dimensions and
  plane bytes survive the round trip bit-for-bit.
* `threshold` — synthesises an 8-bit grayscale plane from fuzz-controlled
  small dimensions + a fuzz-controlled threshold, runs
  `encode_wbmp_from_threshold`, decodes the produced file, and asserts
  (a) the packed bits match a bit-by-bit reference that walks the
  grayscale buffer column-by-column setting bit `7 - x%8` whenever
  `gray[y*w + x] >= threshold`, and (b) the padding bits in the last
  byte of every row are zero regardless of the input grayscale values.
  This covers the only public entry point with non-trivial per-pixel
  logic that the other two targets miss — the chunked-eight-pixels-
  per-output-byte main loop plus the 1..=7-pixel tail branch.
* `dither` — synthesises an 8-bit grayscale plane from fuzz-controlled
  small dimensions, runs `encode_wbmp_from_dither` (Floyd–Steinberg
  error-diffusion), decodes the produced file, and asserts (a)
  dimensions / stride survive the round trip, (b) the padding bits in
  the last byte of every row are zero — the dither path writes its
  output via `row_out[x >> 3] |= bit` and must never touch the padding
  tail — and (c) the saturated-input agreement against
  `encode_wbmp_from_threshold(.., 128)`: after clamping every input
  sample to `{0, 255}` the two helpers must produce byte-identical
  files, since saturated samples propagate zero residual. Covers the
  only stateful per-pixel encoder path (i16 accumulator + per-row
  cur/next swap with `saturating_add` clamping) — failure modes the
  other three targets miss.
* `polarity` — synthesises a canonical (padding-bit-masked) `MonoWhite`
  plane from fuzz-controlled small dimensions, encodes it, decodes it
  twice (once verbatim via `parse_wbmp`, once polarity-flipped via
  `parse_wbmp_as(MonoBlack)`), and asserts (a) the verbatim decode
  matches the input plane byte-for-byte, (b) the polarity-flipped
  plane equals the inverted-and-padding-masked reference computed from
  the input plane, and (c) the trailing padding bits in the last byte
  of every row of the `MonoBlack` plane are zero. Covers the in-place
  bit-inversion + per-row trailing-padding-bit re-zero logic in
  `parse_wbmp_as` — the only entry point with non-trivial per-byte
  mutation logic that the other four targets don't reach. The
  failure modes it catches that the others would miss are off-by-one
  errors in the per-row "last byte of the row" indexing during the
  in-place padding mask, skipping the mask when `pad_bits == 0`
  (full-byte width), and conditional-mask boundary errors when
  `pad_bits` is 1 or 7.
* `header_ext` — feeds arbitrary bytes to `parse_header_ext`, the
  general-form header parser (WAP-237 §4.4.1–§4.4.3) that decodes the
  `FixHeaderField` bitfields and, when the bit-7 presence flag is set,
  the variable-length `ExtFields` region before reading the
  `Width`/`Height` MBIs. Asserts (a) the call always returns a `Result`
  and never panics / overflows / reads past the slice, (b) a successful
  parse reports non-zero dimensions and a `data_offset` within the
  input, (c) the parsed `ExtFields` option matches the FixHeaderField
  bit-7 flag, and (d) any decoded `ExtFields` survives a
  `write_ext_fields` → `parse_ext_fields` round trip (same region, same
  consumed byte count) whenever the region is writer-representable.
  Covers the extension-header state machine — the 2-bit type selector
  between the type-00 continuation-bit bitfield chain, the type-01/10
  single reserved octets, and the type-11 parameter/value-pair chain
  with attacker-chosen per-pair identifier/value sizes, plus the
  `MAX_EXT_FIELD_BYTES` chain caps and the offset-advance arithmetic
  feeding the trailing dimension MBIs — the most attacker-driven control
  flow in the crate.
* `decode_ext` — feeds arbitrary bytes to `parse_wbmp_ext`, the
  extension-header-aware full decode path. It is the only target that
  walks a fuzz-controlled-length `ExtFields` region and then performs
  the pixel-body length check + verbatim row copy whose `data_offset`
  begins past that variable region. Asserts the call always returns a
  `Result` without panicking / overflowing / reading past the slice,
  and that a successful decode yields exactly one packed plane whose
  data length equals `stride * height`. Covers `decode_body`'s
  post-ExtFields body slice index, the `total_bytes` vs. body-length
  comparison, and the limit checks applied to dimensions read after the
  ExtFields — none of which `header_ext` (header-only) reaches.

All seven build with `default-features = false`, so the harness
exercises the framework-free standalone path and never links
`oxideav-core`. Run:

```sh
cargo +nightly fuzz run decode
cargo +nightly fuzz run roundtrip
cargo +nightly fuzz run threshold
cargo +nightly fuzz run dither
cargo +nightly fuzz run polarity
cargo +nightly fuzz run header_ext
cargo +nightly fuzz run decode_ext
```

## Benchmarks

A Criterion suite in [`benches/`](benches/) covers the three hot paths
end-to-end (`decode`, `encode`, full encode-→-decode `roundtrip`) at
six representative sizes: 8×8 (per-call overhead), 96×64 (WAP-era
handset), 320×240 (QVGA, 2-byte width MBI), 159×33 (odd-width padding-
bit boundary), 1024×1024 (mid-size wallpaper) and 2048×2048
(largest fixture still admitted by the default `WbmpLimits`
8 MiB pixel cap). The `encode` bench also exercises
`encode_wbmp_from_threshold` and `encode_wbmp_from_dither` on a
320×240 grayscale fixture — the two per-pixel hot loops in the
encoder, useful as an A/B for tracking the dither path's per-pixel
cost separately from the threshold branch-and-set loop.

Each scenario synthesises its fixture in-process from a deterministic
xorshift32 source (no fixture files on disk) so the harness stays
self-contained. Run with:

```sh
cargo bench -p oxideav-wbmp --bench decode
cargo bench -p oxideav-wbmp --bench encode
cargo bench -p oxideav-wbmp --bench roundtrip
```

Indicative numbers on an Apple M1 Pro (release, single core): decode
tops out around 71 GiB/s on the 2048×2048 fixture (memory-copy bound),
encode at 60 GiB/s on the 1024×1024 fixture, end-to-end roundtrip at
22 GiB/s on 1024×1024, and `encode_wbmp_from_threshold` at ~10 GiB/s on
the 320×240 Gray8 fixture. The dither path is dominated by the
inherently-sequential Floyd–Steinberg residual diffusion, so its
headline throughput stays an order of magnitude below the threshold
path.

## Framework trait surface

The default-on `registry` feature exposes the codec / container behind
`oxideav-core`'s `Decoder`, `Encoder`, `Demuxer` and `Muxer` traits.
[`tests/round13_registry_traits.rs`](tests/round13_registry_traits.rs)
covers that surface end-to-end: round-trips a `MonoWhite` plane
through `WbmpDecoder::send_packet` → `receive_frame`; routes the same
plane back through `WbmpEncoder::send_frame` → `receive_packet`;
asserts the `MonoBlack` polarity path performs the in-place inversion
+ per-row padding-mask documented in the encoder source; checks
`Gray8` thresholds at 128 by default through the framework path;
exercises the `NeedMore` / `Eof` semantics on both directions; calls
`probe` directly with conformant, garbage and extension-only inputs;
opens a `WbmpDemuxer` / `WbmpMuxer` pair via
`ContainerRegistry::open_demuxer` / `open_muxer` and round-trips the
single-packet container; confirms the muxer rejects audio and
multi-stream inputs; and walks `register_codecs` to assert
`CodecCapabilities` advertises `MonoWhite`, `MonoBlack` and `Gray8`
as accepted pixel formats with `intra_only` + `lossless` set. These
integration tests are framework-only — the standalone build
(`--no-default-features`) skips them as expected.

## Not supported

* WBMP Type values other than `0`. Later WAP releases reserved Type 1+
  for greyscale / colour bitmaps but never published a normative
  encoding, and no public devices shipped non-Type-0 content. Other
  Type values raise `WbmpError::Unsupported`.
* **Animation timing.** The animated sub-image *frames* are decoded
  (see [Animated sub-images](#animated-sub-images)), but WAP-237 defines
  no normative animation timing parameters — §4.5.1 leaves it "User
  Agent dependent how those animated images are processed" — so the
  crate surfaces the raw frame planes and does not impose a frame rate.

## License

MIT. See `LICENSE`.
