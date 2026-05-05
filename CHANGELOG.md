# Changelog

All notable changes to this crate are tracked here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial round-1 implementation: WBMP Type 0 (uncompressed B/W bitmap)
  reader + writer, clean-room from the WAP Forum WAP-237 specification.
- Multi-Byte Integer (MBI) codec helpers for variable-length unsigned
  integers (high-bit continuation, big-endian payload bits).
- Standalone `parse_wbmp` / `encode_wbmp` / `encode_wbmp_from_threshold`
  API plus default-on `registry` feature wiring `Decoder` / `Encoder` /
  container `Demuxer` / `Muxer` trait impls against `oxideav-core`.
