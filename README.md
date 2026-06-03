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
in [`fuzz/`](fuzz/) with four libFuzzer targets:

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

All four build with `default-features = false`, so the harness
exercises the framework-free standalone path and never links
`oxideav-core`. Run:

```sh
cargo +nightly fuzz run decode
cargo +nightly fuzz run roundtrip
cargo +nightly fuzz run threshold
cargo +nightly fuzz run dither
```

Round-1 sweep (~45 M `decode` + ~8 M `roundtrip` executions) found
no crashes; RSS stayed under ~530 MiB throughout, confirming the
allocation guards hold against adversarial headers. Round-7 added the
`threshold` target and ran a 60-second smoke sweep: 1.15 M executions,
no crashes, RSS bounded at ~471 MiB, libFuzzer coverage saturated at
319 features / 1630 ft within the first ~5 s — the encode-side
arithmetic and per-row indexing are panic-free across every reachable
input shape the fuzzer explored. Round-9 added the `dither` target on
the same pattern: 60-second smoke sweep, no crashes, RSS bounded under
~500 MiB, the Floyd–Steinberg accumulator's `saturating_add` clamps
hold across every reachable input shape the fuzzer explored and the
saturated-input agreement against `encode_wbmp_from_threshold(.., 128)`
holds byte-for-byte on every clamped-to-{0,255} probe.

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

Round-1 numbers on an Apple M1 Pro (release, single core) for context:
decode tops out around 71 GiB/s on the 2048×2048 fixture (memory-copy
bound), encode at 60 GiB/s on the 1024×1024 fixture, end-to-end
roundtrip at 22 GiB/s on 1024×1024. Round-5 pushed the
`encode_wbmp_from_threshold` path to ~10 GiB/s on the 320×240 Gray8
fixture by replacing the per-pixel `|= 1 << k` read-modify-write loop
with a packed eight-comparisons-per-output-byte loop that lets the
codegen unroll the inner step cleanly. Round-10 applied the same
accumulator-flush pattern to `encode_wbmp_from_dither`'s per-row inner
loop (~140 → ~142 MiB/s on the same 320×240 fixture; the dither path
is dominated by the inherently-sequential Floyd–Steinberg residual
diffusion, so the headline number stays an order of magnitude below
the threshold path, but the structural alignment removes one
read-modify-write per pixel and matches the threshold path's
per-byte-store granularity).

## Round 1 deferrals

* WBMP Type values other than `0`. Later WAP releases reserved Type 1+
  for greyscale / colour bitmaps but never published a normative
  encoding, and no public devices shipped non-Type-0 content. If
  someone ever produces a real Type-N fixture this can be revisited.
