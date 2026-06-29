# Changelog

All notable changes to this crate are tracked here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `parse_header_ext_strict` — the strict counterpart to
  `parse_header_ext`, a fully-conformant general-form (§4.4.1) header
  parse. It tightens the extension-aware path the same way
  `parse_header_strict` tightens the plain four-field path: every MBI
  (Type, Width, Height) must use the §4.3.1 shortest encoding (no leading
  `0x80`), and a Type-11 `ExtFields` region is decoded through
  `parse_ext_fields_strict` so its parameter pairs are character-class
  validated. It deliberately keeps accepting a non-`0x00` FixHeaderField
  with the presence bit set (otherwise the extension-aware strict path
  would contradict itself); the "FixHeaderField MUST be `0x00`, no
  ExtFields" Type-0 conformance check remains `parse_header_strict`.
  Re-exported at the crate root.
- Strict Type-11 extension-field character-class validation (WAP-237
  §4.4.3 / §4.2 ABNF, RFC 2234 conventions). The normative grammar is
  `ParameterIdentifier = 1*8CHAR` (US-ASCII `CHAR` = `%x01-7F`) and
  `ParameterValue = 1*16(ALPHA / DIGIT)` (`A-Za-z0-9`); the lax
  `parse_ext_fields` stores the bytes verbatim regardless of class
  (tolerant decode), while the new `parse_ext_fields_strict` rejects any
  identifier byte outside US-ASCII `CHAR` or value byte outside
  `ALPHA / DIGIT` as `WbmpError::InvalidData` naming the offending byte
  and class. `write_ext_fields_strict` is the matching writer guard — it
  validates every Type-11 `Parameter` against the same classes before
  emitting, so a strict writer can never produce an ABNF-violating
  stream. Type-00 / 01 / 10 regions carry opaque reserved octets with no
  character-class constraint, so the strict and lax paths agree
  byte-for-byte there. New `Parameter::new` validating constructor,
  `Parameter::validate`, and `Parameter::identifier_str` /
  `value_str` UTF-8 accessors round out the surface. All four entry
  points are re-exported at the crate root. (The 3-/4-bit
  `ParameterHeader` size fields already cap the lengths at 7 / 15, so the
  strict check adds only the character-class constraint on top of the
  length bounds both paths enforce.)
- Fourth Criterion bench `frames` (`benches/frames.rs`), covering the
  multi-frame animation path `encode_wbmp_frames` → `parse_wbmp_frames`
  (WAP-237 §4.2 / §4.5.1) that the `decode` / `encode` / `roundtrip`
  benches never reach — every one of those stops at a single frame. It
  sweeps the frame count (96×64 × {1, 4, 16} and 320×240 × 8) so the
  per-frame marginal cost (one length check plus a verbatim plane copy)
  is measurable against the fixed single-header overhead that amortises
  as frames are added; throughput is byte-keyed on the summed plane size.
  Fixtures are synthesised in-process from a deterministic xorshift32
  source (no fixture files). Indicative Apple-M1-class numbers: ~6.7 GiB/s
  on single-frame 96×64, rising toward 14–18 GiB/s as the frame count
  grows. Registered as the `frames` bench in `Cargo.toml` (`harness =
  false`).
- Eighth cargo-fuzz target `frames`, covering the animated-sub-image
  entry points `parse_wbmp_frames` / `encode_wbmp_frames` (WAP-237 §4.2 /
  §4.5.1) — the only public surface the prior seven targets never reach.
  Two halves share one input: (1) the **decode** half feeds arbitrary
  bytes to `parse_wbmp_frames` and asserts it always returns a `Result`
  (no panic, no debug-overflow, no out-of-bounds slice), and on a
  successful decode that the `WbmpAnimation` is self-consistent — 1..=`1 +
  MAX_ANIMATED_IMAGES` frames, every plane exactly `stride * height`
  bytes, `animated_count` / `is_animated` agreeing with `frames.len()`,
  and `main_image()` == `frames[0]` == the single-frame `parse_wbmp`
  plane of the same buffer; (2) the **encode** half synthesises 1..=16
  distinct same-dimension packed planes from the remaining fuzz bytes,
  round-trips them through `encode_wbmp_frames` → `parse_wbmp_frames`, and
  asserts every plane survives byte-for-byte **in stream order** (so a
  frame-ordering or back-to-back-layout bug surfaces as a mismatch), plus
  the documented single-frame `encode_wbmp` byte-equivalence. Registered
  as the `frames` bin in `fuzz/Cargo.toml` with a catalog-comment entry.
- Animated sub-image **encoding** (WAP-237 §4.2 / §4.5.1) — the inverse
  of `parse_wbmp_frames`. New `encode_wbmp_frames(width, height, &[main,
  anim…])` writes the single four-field header followed by the main image
  plane and 0..15 same-dimension animated sub-image planes back-to-back
  (no per-frame header, per the §4.2 BNF `Image-data = Main-image
  0*15Animated-image`). Every frame must be exactly `ceil(width/8) *
  height` bytes; the §4.5.1 cap (`1 + MAX_ANIMATED_IMAGES == 16` frames)
  and zero-dimension / empty-list / wrong-size-frame inputs raise
  `WbmpError::InvalidData`. A single-element call is byte-identical to
  `encode_wbmp`, and a `encode_wbmp_frames` → `parse_wbmp_frames` round
  trip recovers every plane in stream order. Six new unit tests in
  `src/encoder.rs` cover single-frame equivalence, the three-frame round
  trip (multi-byte rows / padding tail), the §4.5.1 max-accepted /
  too-many-rejected boundary, and the empty / zero-dim / wrong-size error
  paths. `encode_wbmp_frames` re-exported from the crate root.
- Animated sub-image decoding (WAP-237 §4.2 / §4.5.1). The §4.2 BNF is
  `Image-data = Main-image 0*15Animated-image` and §4.5.1 states *"The
  WBMP image can have at most 15 animated images following the main
  image."* Each animated sub-image is a bitmap "formed according to image
  data structure specified by the TypeField" — for Type 0 that is an
  identically-dimensioned packed 1-bit plane (no per-frame header; the
  single header `Width`/`Height` govern every frame), so each occupies
  exactly `stride * height` bytes. New `parse_wbmp_frames` /
  `parse_wbmp_frames_with_limits` entry points return a `WbmpAnimation`
  carrying the main image plus any trailing frames (`frames[0]` is the
  main image; `frames[1..]` the animated sub-images in stream order),
  capped at `MAX_ANIMATED_IMAGES` (15) animated frames per §4.5.1. A
  trailing run shorter than one full frame is treated as ignorable
  padding (matching the existing `parse_wbmp` posture toward trailing
  bytes), and the per-frame `WbmpLimits` checks reuse the same
  dimension / pixel-byte guards as the single-frame path. `WbmpAnimation`
  exposes `animated_count`, `is_animated` and `main_image` helpers. The
  existing single-frame `parse_wbmp` is unchanged. Eleven new unit tests
  in `src/decoder.rs` cover single-frame equivalence, multi-frame decode,
  multi-byte-row frames, partial-trailing-run handling, the §4.5.1
  frame-count cap, truncated-main / non-zero-type / limit-exceeded error
  paths, unbounded-limit large-frame decode, and a fuzz sweep. New
  `parse_wbmp_frames`, `parse_wbmp_frames_with_limits`, `WbmpAnimation`
  and `MAX_ANIMATED_IMAGES` re-exported from the crate root.
- Shortest-encoding MBI conformance on the strict decode path
  (WAP-237 §4.3.1). The spec is explicit: *"The unsigned integer MUST
  be encoded in the smallest encoding possible. In other words, the
  encoded value MUST NOT start with an octet with the value 0x80."* A
  new `mbi::read_mbi_u32_strict` reader enforces that MUST NOT — a
  leading `0x80` octet (a redundant, no-payload continuation byte) is
  rejected as `WbmpError::InvalidData`, while an *interior* `0x80` group
  (e.g. the middle octet of `0x4000` = `0x81 0x80 0x00`) is still
  accepted, since only a *leading* `0x80` is forbidden. The strict
  header / decode entry points (`parse_header_strict`,
  `parse_wbmp_strict`, `parse_wbmp_strict_with_limits`) now route their
  Type / Width / Height MBI reads through the strict reader, so a strict
  parse rejects redundantly-padded dimension fields in addition to the
  existing non-`0x00` `FixedHeader` rejection. The lax
  `read_mbi_u32` / `parse_header` / `parse_wbmp` paths are unchanged —
  they continue to tolerate a bounded amount of leading-`0x80` padding
  for forward-compat with files seen in the wild. `read_mbi_u32_strict`
  is re-exported from the crate root. Eleven new unit tests across
  `src/mbi.rs` (leading-`0x80` rejection, interior-`0x80` acceptance,
  offset-relative leading-octet check, inherited truncation / overflow
  guards), `src/header.rs` (strict rejection of padded Type / Width
  MBIs, acceptance of minimal MBIs with an interior `0x80`), and
  `src/decoder.rs` (full strict-decode path rejecting a padded Width
  MBI behind a conformant `0x00` FixedHeader). No wire-format change to
  the conformant encoder, which already emits shortest encodings.
- Extension-header-aware decode entry points `parse_wbmp_ext` /
  `parse_wbmp_ext_with_limits`, returning a new `WbmpImageExt`
  (`{ image: WbmpImage, ext_fields: Option<ExtFields> }`). The prior
  round added `parse_header_ext` (header fields only) but no decode
  path turned an `ExtFields`-bearing stream into a `WbmpImage`: the
  public `parse_wbmp` uses the plain four-field `parse_header`, so a
  (non-conformant) Type-0 file carrying extension headers had its first
  `ExtField` octet mis-read as the `Width` MBI. The new entry points
  route through `parse_header_ext`, consuming any `ExtFields` region
  (WAP-237 §4.4.1–§4.4.3) before reading the dimensions, then decode
  the §4.5.1 main image data via the shared `decode_body` helper (same
  limit checks, plane-layout computation and verbatim row copy as the
  plain path). For a conformant Type-0 file (`FixHeaderField == 0x00`)
  the decoded image is byte-identical to `parse_wbmp` and `ext_fields`
  is `None`. Six new unit tests cover the conformant equivalence, image
  decode after both a Type-11 parameter-pair region and a Type-00
  bitfield chain, limit enforcement, truncated pixel data, and a
  truncated ExtFields region.
- Seventh `cargo-fuzz` target `decode_ext` driving `parse_wbmp_ext` —
  the only target that walks a fuzz-controlled-length `ExtFields`
  region and then performs the pixel-body length check + verbatim row
  copy whose `data_offset` begins past that variable region. It
  asserts the call always returns a `Result` without panicking /
  overflowing / reading past the slice, and that a successful decode
  yields one packed plane whose length equals `stride * height`.
- Round-296 hardening: sixth `cargo-fuzz` target `header_ext`
  exercising the general-form header parser `parse_header_ext`
  (WAP-237 §4.4.1–§4.4.3) over arbitrary bytes. It drives the
  `FixHeaderField` bitfield split, all four `ExtFields` type branches
  (the type-00 continuation-bit bitfield chain, the type-01/10 single
  reserved octets, and the type-11 parameter/value-pair chain with
  attacker-chosen per-pair identifier/value sizes), the
  `MAX_EXT_FIELD_BYTES` chain caps, and the offset-advance arithmetic
  feeding the trailing `Width`/`Height` MBIs — the most attacker-driven
  control flow in the crate, reached by none of the prior five targets
  (they use the opaque-`FixHeaderField` `parse_header` or the encoder
  paths). The target asserts the call always returns a `Result` and
  never panics / overflows / reads past the input slice; that a
  successful parse reports non-zero dimensions and a `data_offset`
  within the input; that the parsed `ExtFields` option matches the
  FixHeaderField bit-7 presence flag; and that any writer-representable
  decoded `ExtFields` region survives a `write_ext_fields` →
  `parse_ext_fields` round trip (same region, same consumed byte
  count). A 3.0 M-execution sweep (~11 s, `-max_len=512`) found no
  crashes, RSS bounded at ~553 MiB, libFuzzer feature coverage
  saturated at 235 features / 578 ft. No `src/` change was required.
- Extension-header (`ExtFields`) parsing — WAP-237 §4.4.1–§4.4.3 — in
  the new `src/ext.rs` module. The general WBMP header format
  (`Header = TypeField FixHeaderField [ExtFields] Width Height`)
  permits an optional extension-header region between the
  FixHeaderField and the Width MBI; the FixHeaderField's bit-7
  presence flag (Table 4-3) signals it and bits 6-5 select its type.
  The module decodes the FixHeaderField bitfields (`FixHeaderField`,
  `ExtFieldType`) and all four ExtFields layouts: Type 00 multi-byte
  reserved bitfield (bit-7 continuation chain), Type 01 / Type 10
  single reserved octet, and Type 11 parameter/value-pair sequence
  (`ParameterHeader` = concat flag | 3-bit identifier size 1-8 |
  4-bit value size 1-16, followed by the US-ASCII identifier and
  alphanumeric value, §4.4.3 Table 4-4). Two public entry points:
  `parse_ext_fields` (decode a region given a `FixHeaderField`) and
  `write_ext_fields` (the inverse serializer, for round-trip / hand
  construction). A new `header::parse_header_ext` returns the richer
  `HeaderExt` — width/height/data_offset plus the decoded
  FixHeaderField and `Option<ExtFields>` — honouring the presence flag
  so the decoder lands on the real Width/Height instead of mis-reading
  the first ExtField octet as the width MBI when a (non-conformant)
  Type-0 file carries extension headers. A `MAX_EXT_FIELD_BYTES`
  ceiling (4096) bounds pathological all-continuation chains. The
  existing lax/strict `parse_header` / `parse_wbmp` paths are
  unchanged (Type 0 conformantly fixes FixHeaderField at `0x00`, so
  there are never ExtFields in shipped files); the new path is purely
  additive. 31 new unit tests in `src/ext.rs` and 6 in `src/header.rs`
  cover every layout, the write/parse round trip, truncation, the
  zero-size / oversize-length rejections, and the byte-cap guards. No
  wire-format change to the conformant encoder.
- Typed primitive `PlaneLayout` in `src/image.rs` capturing the
  byte-level layout of a single packed mono plane: `width`, `height`,
  `stride` (= `ceil(width / 8)`), `total_bytes` (= `stride * height`,
  with `checked_mul` for usize-overflow safety on 32-bit targets), and
  `last_byte_pad_mask` (= `0xFF` for byte-aligned widths,
  `0xFF << (8 * stride - width)` otherwise). Constructed once via
  `PlaneLayout::new(width, height)` and consumed by four call sites
  that previously each rederived the same three quantities — the lax /
  strict header decoder path (`decoder::parse_wbmp_inner`), the
  polarity-flip path (`decoder::invert_plane_in_place`), the
  standalone encoder (`encoder::encode_wbmp`), and the registry-side
  `MonoBlack` ingress branch in `WbmpEncoder::send_frame`. The encoder
  and decoder polarity-flip branches now share the exact same mask
  byte by querying the same `PlaneLayout::last_byte_pad_mask` field
  rather than each computing `0xFF << (8 * stride - width)` from
  scratch, so any future change to the padding convention only has to
  edit one struct field. Five unit tests in `src/image.rs::tests`
  cover the byte-aligned / partial-byte / zero-dimension /
  total-bytes-matches-row-stride-times-height / 32-bit-overflow
  cases. No wire-format or public-API behaviour change.
- Round-13 coverage: twenty new integration tests in
  `tests/round13_registry_traits.rs` exercising the framework trait
  surface end-to-end — the `Decoder` / `Encoder` / `Demuxer` / `Muxer`
  paths that previous rounds covered only by `cargo build`'s type-check.
  The standalone-API integration tests in `tests/roundtrip.rs` drove
  the framework-free `parse_wbmp` / `encode_wbmp[_from_*]` entry
  points only; the `#[cfg(feature = "registry")]` paths in
  `src/decoder.rs`, `src/encoder.rs`, `src/container.rs` and
  `src/registry.rs` had no runtime coverage. Round 13 plugs that gap
  across five focused groups: (1) `register_codecs` capability shape
  asserting `MonoWhite` / `MonoBlack` / `Gray8` are all advertised
  with `intra_only = true` and `lossless = true`; (2) `WbmpDecoder`
  `send_packet` → `receive_frame` round-trips in both `MonoWhite`
  (verbatim) and `MonoBlack` (in-place inverted + padding-masked)
  polarities, including the `NeedMore` / `Eof` state machine and the
  `WbmpError` → `oxideav_core::Error` conversion path; (3)
  `WbmpEncoder` `send_frame` → `receive_packet` for `MonoWhite`,
  `MonoBlack` (with padding re-zeroed on disk) and `Gray8`
  (thresholded at 128), plus the `NeedMore` / `Eof` semantics and the
  unsupported-format / missing-pixel-format rejection branches; (4)
  the `container::probe` function — full `PROBE_SCORE_EXTENSION` on
  matching `.wbmp` hints, `PROBE_SCORE_EXTENSION / 2` on conformant
  content sniffs without hints, zero on obvious non-WBMP buffers
  (JPEG SOI) and on buffers shorter than the 5-byte minimum the
  probe demands; (5) `ContainerRegistry::open_demuxer` /
  `open_muxer` end-to-end — demuxing a real Type-0 file emits the
  expected single packet with `pts = Some(0)` and `keyframe = true`,
  the streams metadata advertises `MediaType::Video` + the on-disk
  `PixelFormat::MonoWhite`, garbage input surfaces as a clean
  `InvalidData` / `Unsupported` error, and the muxer rejects both
  audio streams and multi-stream inputs as documented. Tests gated
  behind `#[cfg(feature = "registry")]` so the standalone build
  (`--no-default-features --lib`) is unaffected; 94 tests total now
  pass (63 unit + 11 standalone integration + 20 trait-surface
  integration). `container::probe` promoted from private to `pub`
  to make it callable from integration tests without going through
  the `ContainerRegistry::probe_input` machinery; the function shape
  and behaviour are unchanged.
- Round-12 API surface: strict header-conformance entry points
  `parse_wbmp_strict` / `parse_wbmp_strict_with_limits` (high-level)
  and `parse_header_strict` (low-level). The strict variants require
  the wire-format `FixedHeader` byte to be the spec-mandated `0x00`;
  the lax `parse_wbmp` / `parse_header` continue to accept any value
  for forward-compatibility with hypothetical Type-0 extensions. A
  non-conformant byte raises `WbmpError::InvalidData` with a message
  naming the offending byte and the mode (e.g. `"FixedHeader byte =
  0xFF, strict mode requires 0x00"`). All other rejection paths
  (Type-field, zero-dimension, MBI bounds, limits, truncation) match
  the lax decoder exactly — strict is an ADDITIONAL check, not a
  replacement, and limits checks still fire on dimensions that pass
  the strict FixedHeader test. Seven new unit tests cover the
  conformant-passes parity, the FixedHeader-rejection branch (full
  byte, high-bit-only), the strict-orderings against limits / Type /
  zero-dim / truncation, and the `parse_header_strict` vs
  `parse_wbmp_strict` API symmetry. No wire-format changes.
- Round-11 hardening: fifth `cargo-fuzz` target `polarity` exercising
  `parse_wbmp_as(MonoBlack)` end-to-end. The fuzzer synthesises a
  canonical (trailing-padding-bit-pre-masked) `MonoWhite` plane from
  fuzz-controlled small dimensions (1..=256 on each axis to stay under
  the default `WbmpLimits`) plus a fuzz-controlled body, encodes it,
  decodes it twice — once verbatim via `parse_wbmp` and once
  polarity-flipped via `parse_wbmp_as(MonoBlack)` — and asserts (a)
  the verbatim decode matches the input plane byte-for-byte, (b) the
  polarity-flipped plane equals the inverted-and-padding-masked
  reference computed from the input plane, and (c) the trailing
  padding bits in the last byte of every row of the `MonoBlack` plane
  are zero regardless of the input pattern. Covers the in-place bit-
  inversion + per-row trailing-padding-bit re-zero logic in
  `parse_wbmp_as` — the only entry point with non-trivial per-byte
  mutation logic that the existing four targets (`decode`,
  `roundtrip`, `threshold`, `dither`) don't reach. Failure modes the
  new target catches: off-by-one in per-row "last byte of the row"
  indexing during the in-place padding mask, skipping the mask when
  `pad_bits == 0` (full-byte width), conditional-mask boundary errors
  when `pad_bits` is 1 or 7. Initial 60-second sweep on Apple M1 Pro:
  8.3 M executions, no crashes, RSS bounded at ~443 MiB, libFuzzer
  feature coverage saturated at 195 features / 510 ft inside the first
  ~5 s. Builds with `default-features = false` (no `oxideav-core`
  link), same shape as the other four targets.

### Changed
- Round-10 perf: `encode_wbmp_from_dither`'s per-row inner loop now
  accumulates the eight output bits of each byte into a `u8` register
  and stores once per byte rather than doing a read-modify-write
  `row_out[x >> 3] |= bit << shift` on every pixel. The bit positions
  never collide (each pixel writes exactly bit `7 - (x & 7)` of byte
  `x >> 3`), so the change produces a byte-identical plane to the
  previous form. A partial-byte flush handles the `width % 8 != 0`
  tail, with the unused low bits left zero by construction (matching
  the WBMP padding convention). Two new dedicated unit tests pin the
  exact output bytes: `dither_helper_full_byte_plus_tail_bits` exercises
  an 11×1 saturated checkerboard (one full byte plus three tail bits)
  and `dither_helper_byte_boundary_padding_stays_zero` exercises a 9×1
  saturated-white input that lands one bit in byte 1 and seven zero
  padding bits after it. Measured speedup at `encode_wbmp_from_dither`
  on the 320×240 Gray8 Criterion bench: ~525 µs → ~514 µs (~2.0%
  Criterion-reported `p < 0.05`, ~140 → ~142 MiB/s) on Apple M1 Pro
  release single-core. The structural alignment with
  `encode_wbmp_from_threshold`'s chunked-eight pack matters more than
  the headline number: future changes to either encoder hot loop can
  now be compared at the same per-byte-store granularity rather than
  one chunked + one read-modify-write.

### Added
- Round-9 hardening: fourth `cargo-fuzz` target `dither` exercising
  `encode_wbmp_from_dither` end-to-end. The fuzzer drives small
  dimensions (1..=256 on each axis to stay under the default
  `WbmpLimits`), synthesises a grayscale buffer cycled from the
  remaining fuzz input, runs the dither encoder, decodes the result,
  and asserts (a) dimensions / stride survive the round trip, (b) the
  padding bits in the last byte of every row are zero regardless of
  input grayscale values, and (c) the saturated-input agreement
  against `encode_wbmp_from_threshold(.., 128)`: after clamping every
  input sample to `{0, 255}` the two helpers produce byte-identical
  files (saturated samples propagate zero residual under
  Floyd–Steinberg, so the two helpers are documented to match on
  pre-quantised input). Covers the only stateful per-pixel encoder
  path — i16 accumulator, signed-divide rounding on the residual,
  per-row `cur`/`next` buffer swap with `saturating_add` clamping —
  that the existing `threshold` target's stateless branch-and-set
  loop doesn't reach. Builds with `default-features = false` (no
  `oxideav-core` link), same shape as the other three targets.
- Round-9 depth-mode: companion Criterion benchmark
  `encode_dither_320x240_gray8` in `benches/encode.rs` covering the
  Floyd–Steinberg path on the same 320×240 Gray8 fixture the
  `encode_threshold_320x240_gray8` bench uses, so future encoder
  changes can A/B the dither cost against the threshold cost
  separately rather than as a single aggregate per-pixel number.
- Round-8 API surface: new `encode_wbmp_from_dither(width, height, gray)`
  helper that runs a Floyd–Steinberg error-diffusion quantiser over an
  8-bit grayscale input before packing the resulting 1-bit plane into
  a Type-0 file. Photographic mid-tones now land as a stippled pattern
  that preserves local average luminance rather than collapsing to a
  flat region as the `encode_wbmp_from_threshold` cut-off does. The
  implementation uses an i16 row-accumulator (O(width) extra space)
  and the classic 7/16, 3/16, 5/16, 1/16 forward-neighbour weight
  distribution; saturated 0/255 inputs diffuse zero residual, so
  flat-monochrome input agrees byte-for-byte with
  `encode_wbmp_from_threshold(.., 128)` and the two helpers stay
  interchangeable on already-quantised data. Six new unit tests cover
  the pass-through, zero-dim rejection, size-mismatch rejection,
  flat-128 mid-tone balance (45–55% white bits on a 32×32 patch),
  width-with-padding roundtrip (padding bits zero), and a 64-pixel
  horizontal ramp landing within ±10% of half-and-half. Reference:
  R. W. Floyd and L. Steinberg, "An adaptive algorithm for spatial
  greyscale", *Proc. SID* 17/2 (1976), pp. 75–77.
- Round-7 hardening: third `cargo-fuzz` target `threshold` exercising
  `encode_wbmp_from_threshold` end-to-end. The fuzzer drives small
  dimensions (1..=256 on each axis to stay under the default
  `WbmpLimits`) plus a fuzz-controlled threshold, synthesises a
  grayscale buffer cycled from the remaining fuzz input, runs the
  threshold-encoder, decodes the result, and asserts (a) the packed
  bits match a bit-by-bit reference, and (b) the per-row padding bits
  are zero regardless of input grayscale values. Covers the only
  public entry point with non-trivial per-pixel logic that the
  existing `decode` and `roundtrip` targets don't reach — the
  chunked-eight-pixels-per-output-byte main loop plus the 1..=7-pixel
  tail branch. Initial 60-second sweep on Apple M1 Pro: 1.15 M
  executions, no crashes, RSS bounded at ~471 MiB, libFuzzer feature
  coverage saturated at 319 features / 1630 ft inside the first ~5 s.
  Builds with `default-features = false` (no `oxideav-core` link), same
  shape as the other two targets.

## [0.0.2](https://github.com/OxideAV/oxideav-wbmp/compare/v0.0.1...v0.0.2) - 2026-05-29

### Other

- Round-6 API symmetry: caller-selectable MonoWhite/MonoBlack decode polarity
- Round-5 perf: chunked 8-pixel pack in encode_wbmp_from_threshold
- Round-4 depth-mode: Criterion bench suite
- Round-3 hardening: add cargo-fuzz harness (decode + roundtrip)
- Round-2 hardening: WbmpLimits + tightened MBI cap + adversarial-input sweep

### Added
- Round-6 API symmetry: caller-selectable decode polarity via
  `parse_wbmp_as` / `parse_wbmp_as_with_limits` plus a new
  `WbmpPixelFormat::MonoBlack` variant. The decode path performs the
  bit-inversion + trailing-padding-bit mask in-place during the row
  copy (no extra allocation versus the verbatim path) so consumers
  expecting `1 = black` no longer need to walk the plane themselves
  after decode. The padding-mask is the same one the encoder applies
  on the `MonoBlack` ingress path, so an encode-then-decode through
  matching polarities is bit-exact on the payload bits and zero on the
  padding tail of every row. Under the default-on `registry` feature
  the framework `Decoder` honours
  `CodecParameters::pixel_format = Some(PixelFormat::MonoBlack)` and
  routes through the same in-place transform; the standalone API path
  is unchanged for callers passing `MonoWhite` (the on-disk polarity).
  Coverage: 7 new unit tests covering the full-byte, 5-bit-padding,
  multi-row + limits-propagation cases on top of the existing 48-test
  baseline.
- Initial round-1 implementation: WBMP Type 0 (uncompressed B/W bitmap)
  reader + writer, clean-room from the WAP Forum WAP-237 specification.
- Multi-Byte Integer (MBI) codec helpers for variable-length unsigned
  integers (high-bit continuation, big-endian payload bits).
- Standalone `parse_wbmp` / `encode_wbmp` / `encode_wbmp_from_threshold`
  API plus default-on `registry` feature wiring `Decoder` / `Encoder` /
  container `Demuxer` / `Muxer` trait impls against `oxideav-core`.
- Round-2 hardening: configurable `WbmpLimits` (default `max_width =
  max_height = 16384`, `max_pixel_bytes = 8 MiB`) bound decoder
  allocation against adversarial headers; new `parse_wbmp_with_limits`
  entry point for callers needing larger images (`WbmpLimits::unbounded`
  restores the old behaviour for trusted local input). New
  `WbmpError::LimitExceeded` variant maps to
  `oxideav_core::Error::ResourceExhausted` under `registry`.
- Round-2 hardening: tightened MBI decoder ceiling from 9 to
  `MAX_MBI_BYTES = 7` (5 minimal-encoding bytes + 2-byte 0x80 padding
  allowance); pathological continuation-byte runs now error in O(1)
  rather than chasing the input.
- Round-2 hardening: explicit adversarial-input test sweep — 65 536
  two-byte prefixes plus 4 096 LCG-seeded random buffers driven through
  `parse_wbmp` and shown to never panic.
- Round-3 hardening: `cargo-fuzz` harness (`fuzz/`) with two libFuzzer
  targets — `decode` (panic-free `parse_wbmp` over arbitrary bytes) and
  `roundtrip` (encode → decode bit-exactness over fuzz-controlled
  dimensions + packed bits). Both build `default-features = false`
  (no `oxideav-core` link). Initial sweep (~45 M + ~8 M executions)
  found no crashes; RSS stayed bounded, confirming the `checked_mul`,
  `MAX_MBI_BYTES`, and `WbmpLimits` allocation guards hold.
- Round-5 perf: rewrote `encode_wbmp_from_threshold`'s per-row inner
  loop to pack eight grayscale samples into one output byte in a single
  expression (`((g[0] >= t) as u8) << 7 | … | (g[7] >= t) as u8`),
  eliminating the per-pixel `row_out[x/8] |= 1 << (7 - (x%8))`
  read-modify-write. The full-byte head of the row uses
  `chunks_exact(8)`; a small tail loop covers the trailing 1..=7 pixels
  when `width % 8 != 0`. Bit-exact-equivalent on every existing
  roundtrip test (`threshold_helper_full_grayscale_ramp_roundtrip`,
  `threshold_helper_2d_pattern_roundtrip`) plus a new dedicated
  `threshold_helper_full_byte_plus_tail_bits` test pinning the exact
  byte values for a width-11 input that exercises both code paths in
  the same row. Measured at ~10 GiB/s on a 320×240 Gray8 fixture
  (Apple M1 Pro, release, single core).
- Round-4 depth-mode: Criterion bench suite (`benches/`) covering the
  three hot paths (`decode`, `encode`, `roundtrip`) at six
  representative sizes (8×8 / 96×64 / 320×240 / 159×33 / 1024×1024 /
  2048×2048) plus the `encode_wbmp_from_threshold` per-pixel-branch
  path on a 320×240 Gray8 fixture. Fixtures are synthesised in-process
  from a deterministic xorshift32 source — no fixture files on disk.
  Numbers (Apple M1 Pro, release, single core): decode ~71 GiB/s on
  the 2048×2048 fixture (copy-bound), encode ~60 GiB/s on 1024×1024,
  end-to-end roundtrip ~22 GiB/s on 1024×1024.

### Changed
- `parse_wbmp` now enforces `WbmpLimits::default()`. Inputs declaring
  width or height above 16 384 — or computed pixel-data above 8 MiB —
  return `WbmpError::LimitExceeded` instead of allocating; opt back in
  via `parse_wbmp_with_limits(input, &WbmpLimits::unbounded())`.
