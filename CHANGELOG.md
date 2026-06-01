# Changelog

All notable changes to this crate are tracked here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.3](https://github.com/OxideAV/oxideav-wbmp/compare/v0.0.2...v0.0.3) - 2026-06-01

### Other

- Round-8 API surface: encode_wbmp_from_dither (Floyd-Steinberg)
- Round-7 hardening: third cargo-fuzz target for encode_wbmp_from_threshold

### Added
- Round-8 API surface: `encode_wbmp_from_dither(width, height, gray,
  threshold)` — Floyd–Steinberg error-diffusion sibling of
  `encode_wbmp_from_threshold`. Same input shape and same
  `>= threshold` per-pixel quantisation decision, but the
  quantisation error (`source − target`) is distributed to the four
  canonical 1976 Floyd–Steinberg neighbours (`7/16` right, `3/16`
  below-left, `5/16` below, `1/16` below-right). Pixels at row/column
  edges drop the out-of-bounds share silently. Working buffer is two
  `i16` rows (`4 * width` bytes), independent of image height. The
  long-run average brightness of every local region in the output
  matches the input, so photographic input keeps recognisable detail
  instead of collapsing into white-or-black bands; for inputs that
  are far enough above or below the threshold to never flip a
  quantisation decision the dither path agrees with
  `encode_wbmp_from_threshold` bit-for-bit (pinned in the
  `dither_helper_matches_threshold_at_extremes` test). The framework
  `Encoder` (registry feature) keeps the hard-threshold default on
  `Gray8` so existing consumers are bit-exact unchanged; the dither
  path is exposed only via the standalone API. Coverage: 8 new unit
  tests covering solid-white, solid-black, mid-grey density
  conservation, horizontal-ramp left/right white-density inequality,
  zero-padding in the partial-byte tail, threshold-extreme agreement,
  wrong-size rejection, zero-dimension rejection, and a 17×9 odd-
  width roundtrip. Lib test count: 45 → 53.
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
