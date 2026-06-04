# Changelog

All notable changes to this crate are tracked here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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
