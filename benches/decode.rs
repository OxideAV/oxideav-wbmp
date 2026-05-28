//! Criterion benchmarks for the WBMP decoder hot paths.
//!
//! Round 173 (depth-mode benchmarks): `oxideav-wbmp` is at the per-codec
//! saturation point — Type-0 (the only widely-deployed WBMP variant) has
//! a complete read + write path, the MBI codec is bounded, decoder limits
//! reject pathological headers without allocating, and the cargo-fuzz
//! harness has been exercised for ~45 M `decode` + ~8 M `roundtrip`
//! executions across rounds 171/172. Per the workspace
//! "saturated → fuzz / bench / profile" memo this round wires up
//! `criterion` benches mirroring the bmp / qoi / tta / flac shape so
//! future optimisation rounds can A/B-test changes to the decoder hot
//! paths.
//!
//! This file covers the **decoder**; sibling files cover `encode` and
//! `roundtrip`.
//!
//! Each scenario is self-contained: the bench encodes a fresh WBMP on
//! the fly with the public encoder API and then iterates `parse_wbmp`
//! on the encoded bytes. No fixture files are committed.
//!
//!   - **decode_8x8_solid**: 8×8 single-byte-per-row fixture — the
//!     minimum interesting size; isolates per-call overhead (header
//!     parse + tiny copy) from the body-copy cost.
//!   - **decode_96x64_typical**: 96×64 pattern — a representative
//!     1990s/early-2000s WAP-handset bitmap; both dimensions fit in a
//!     single-byte MBI so the header is the minimum 4 bytes.
//!   - **decode_320x240_qvga**: 320×240 — the QVGA WAP-era display cap;
//!     the width MBI grows to 2 bytes, exercising the mixed-MBI-length
//!     branch in the header parser.
//!   - **decode_1024x1024_padded**: 1024×1024 fixture — covers a
//!     "modern wallpaper" sized WBMP that still fits inside the default
//!     `WbmpLimits` (128 KiB body, well under the 8 MiB cap).
//!   - **decode_159x33_odd_width**: width 159 → 1 padding bit per row;
//!     stresses the per-row padding-bit handling against the rest of
//!     the row-major copy.
//!   - **decode_2048x2048_pixel_cap**: 2048×2048 fixture (524 288 body
//!     bytes) — the largest size still admitted by the default
//!     `max_pixel_bytes = 8 MiB` ceiling; useful to compare against
//!     the smaller fixtures as a bandwidth check on the decoder copy.
//!
//! Run with:
//!     cargo bench -p oxideav-wbmp --bench decode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_wbmp::{encode_wbmp, parse_wbmp, parse_wbmp_with_limits, WbmpImage, WbmpLimits};

/// Cheap deterministic xorshift32 — synthesises pseudo-random bits so
/// the inputs aren't trivially compressible / branch-predictable. WBMP
/// pixel data is just an opaque byte stream the decoder copies through;
/// random bits are as informative as natural-image bits and avoid any
/// branch-history bias.
fn xorshift_byte(state: &mut u32) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state & 0xff) as u8
}

fn build_packed_plane(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let stride = WbmpImage::row_stride(width);
    let total = stride * height as usize;
    let mut data = vec![0u8; total];
    let mut state = seed;
    for byte in data.iter_mut() {
        *byte = xorshift_byte(&mut state);
    }
    // Zero the padding bits in the last byte of each row so the encoded
    // bytes match the convention the encoder writes. The decoder ignores
    // these, but it keeps the inputs canonical for the encode bench too.
    let pad_bits = (stride * 8) - width as usize;
    if pad_bits > 0 {
        let mask: u8 = !((1u16 << pad_bits) - 1) as u8;
        for r in 0..height as usize {
            data[r * stride + (stride - 1)] &= mask;
        }
    }
    data
}

fn encode_fixture(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let bits = build_packed_plane(width, height, seed);
    encode_wbmp(width, height, &bits).expect("encode_wbmp")
}

fn bench_decode_8x8_solid(c: &mut Criterion) {
    // 8×8 → 1-byte stride, 8-byte body; header is the minimum 4 bytes.
    let bytes = encode_fixture(8, 8, 0x1234_5678);
    let mut g = c.benchmark_group("decode_8x8_solid");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/8x8"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

fn bench_decode_96x64_typical(c: &mut Criterion) {
    // 96×64 → 12-byte stride, 768-byte body. Both dimensions fit a
    // single-byte MBI (≤ 0x7F).
    let bytes = encode_fixture(96, 64, 0x2345_6789);
    let mut g = c.benchmark_group("decode_96x64_typical");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/96x64"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

fn bench_decode_320x240_qvga(c: &mut Criterion) {
    // 320×240 → 40-byte stride, 9 600-byte body. Width MBI grows to
    // 2 bytes (320 = 0x140 > 0x7F).
    let bytes = encode_fixture(320, 240, 0x3456_789a);
    let mut g = c.benchmark_group("decode_320x240_qvga");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/320x240"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

fn bench_decode_159x33_odd_width(c: &mut Criterion) {
    // 159×33 → 20-byte stride (1 padding bit per row), 660-byte body.
    // Width MBI = 2 bytes (159 = 0x9F > 0x7F).
    let bytes = encode_fixture(159, 33, 0x4567_89ab);
    let mut g = c.benchmark_group("decode_159x33_odd_width");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/159x33"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

fn bench_decode_1024x1024_padded(c: &mut Criterion) {
    // 1024×1024 → 128-byte stride, 131 072-byte body. Width MBI = 2
    // bytes. Sized to stay well inside the default 8 MiB pixel cap.
    let bytes = encode_fixture(1024, 1024, 0x5678_9abc);
    let mut g = c.benchmark_group("decode_1024x1024_padded");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.sample_size(40);
    g.bench_function(BenchmarkId::from_parameter("wbmp/1024x1024"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

fn bench_decode_2048x2048_pixel_cap(c: &mut Criterion) {
    // 2048×2048 → 256-byte stride, 524 288-byte body. The largest body
    // still admitted by the default `max_pixel_bytes = 8 MiB` cap with
    // headroom. Width MBI = 2 bytes (2048 = 0x800).
    let bytes = encode_fixture(2048, 2048, 0x6789_abcd);
    let mut g = c.benchmark_group("decode_2048x2048_pixel_cap");
    g.throughput(Throughput::Bytes(bytes.len() as u64));
    g.sample_size(20);
    g.bench_function(BenchmarkId::from_parameter("wbmp/2048x2048"), |b| {
        b.iter(|| parse_wbmp(criterion::black_box(&bytes)).expect("decode"));
    });
    g.finish();
}

// Acknowledge `WbmpLimits` / `parse_wbmp_with_limits` as part of the
// public surface (used by the encode + roundtrip benches; pulled here
// so the import block stays uniform across the three files).
#[allow(dead_code)]
fn _unused_limits_marker() -> WbmpLimits {
    let lim = WbmpLimits::unbounded();
    // Force `parse_wbmp_with_limits` to stay used at the crate-bench
    // link level even though the decode bench only exercises the default
    // `parse_wbmp` entry point.
    let _ = parse_wbmp_with_limits(&[], &lim);
    lim
}

criterion_group!(
    benches,
    bench_decode_8x8_solid,
    bench_decode_96x64_typical,
    bench_decode_320x240_qvga,
    bench_decode_159x33_odd_width,
    bench_decode_1024x1024_padded,
    bench_decode_2048x2048_pixel_cap,
);
criterion_main!(benches);
