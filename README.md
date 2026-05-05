# oxideav-wbmp

Pure-Rust WBMP (WAP Bitmap) image codec and container for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Covers
WBMP **Type 0** (uncompressed monochrome bitmap) — the only widely-
deployed WBMP variant — in one self-contained crate. Spec source: the
publicly published WAP Forum specification *WAP-237 Wireless
Application Environment Specification* (May 2001), §8 "Image
Formats". No external implementation source was consulted.

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
| 0    | 1 (1-bit) | 1        | `MonoWhite` (1 = white, MSB-first) |

Other Type values raise `WbmpError::Unsupported`. None ever shipped
in public WAP profiles.

## Encode

[`encode_wbmp`] takes an already-packed mono plane (1 bit per pixel,
MSB-first, 1 = white, rows padded to a byte boundary) and prepends a
Type-0 header. [`encode_wbmp_from_threshold`] thresholds an 8-bit
grayscale buffer (one byte per pixel, no row padding) at the supplied
cut-off and produces a complete WBMP file in one call.

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

## Round 1 deferrals

* WBMP Type values other than `0`. Later WAP releases reserved Type 1+
  for greyscale / colour bitmaps but never published a normative
  encoding, and no public devices shipped non-Type-0 content. If
  someone ever produces a real Type-N fixture this can be revisited.
