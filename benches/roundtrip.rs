//! Criterion benchmarks for the full WBMP encode → decode round trip.
//!
//! Round 173 (depth-mode benchmarks). Companion to `decode.rs` /
//! `encode.rs`. Each iteration measures the combined cost of encoding a
//! freshly-built plane and decoding it back; useful as a single
//! end-to-end number when comparing the WBMP path against other 1-bit /
//! mono codecs in the workspace (PBM-P4, BMP indexed-1, etc.).
//!
//!   - **roundtrip_8x8**: 8×8 minimum-interesting size.
//!   - **roundtrip_96x64**: 96×64 WAP-era handset fixture.
//!   - **roundtrip_320x240**: 320×240 QVGA fixture.
//!   - **roundtrip_159x33**: 159×33 odd-width / padding-bit fixture.
//!   - **roundtrip_1024x1024**: 1024×1024 mid-size fixture.
//!
//! Run with:
//!     cargo bench -p oxideav-wbmp --bench roundtrip

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_wbmp::{encode_wbmp, parse_wbmp, WbmpImage};

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
    let pad_bits = (stride * 8) - width as usize;
    if pad_bits > 0 {
        let mask: u8 = !((1u16 << pad_bits) - 1) as u8;
        for r in 0..height as usize {
            data[r * stride + (stride - 1)] &= mask;
        }
    }
    data
}

fn run_roundtrip(width: u32, height: u32, bits: &[u8]) {
    let encoded = encode_wbmp(width, height, bits).expect("encode");
    let decoded = parse_wbmp(&encoded).expect("decode");
    // Ensure the optimiser keeps the decoded result around — using
    // `black_box` on the value rather than via `assert_eq!` keeps the
    // bench focused on the hot path without dragging in formatting
    // machinery on a failed assertion.
    criterion::black_box(decoded);
}

fn bench_roundtrip_8x8(c: &mut Criterion) {
    let bits = build_packed_plane(8, 8, 0x1234_5678);
    let mut g = c.benchmark_group("roundtrip_8x8");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/8x8"), |b| {
        b.iter(|| run_roundtrip(8, 8, criterion::black_box(&bits)));
    });
    g.finish();
}

fn bench_roundtrip_96x64(c: &mut Criterion) {
    let bits = build_packed_plane(96, 64, 0x2345_6789);
    let mut g = c.benchmark_group("roundtrip_96x64");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/96x64"), |b| {
        b.iter(|| run_roundtrip(96, 64, criterion::black_box(&bits)));
    });
    g.finish();
}

fn bench_roundtrip_320x240(c: &mut Criterion) {
    let bits = build_packed_plane(320, 240, 0x3456_789a);
    let mut g = c.benchmark_group("roundtrip_320x240");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/320x240"), |b| {
        b.iter(|| run_roundtrip(320, 240, criterion::black_box(&bits)));
    });
    g.finish();
}

fn bench_roundtrip_159x33(c: &mut Criterion) {
    let bits = build_packed_plane(159, 33, 0x4567_89ab);
    let mut g = c.benchmark_group("roundtrip_159x33");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/159x33"), |b| {
        b.iter(|| run_roundtrip(159, 33, criterion::black_box(&bits)));
    });
    g.finish();
}

fn bench_roundtrip_1024x1024(c: &mut Criterion) {
    let bits = build_packed_plane(1024, 1024, 0x5678_9abc);
    let mut g = c.benchmark_group("roundtrip_1024x1024");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.sample_size(30);
    g.bench_function(BenchmarkId::from_parameter("wbmp/1024x1024"), |b| {
        b.iter(|| run_roundtrip(1024, 1024, criterion::black_box(&bits)));
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_roundtrip_8x8,
    bench_roundtrip_96x64,
    bench_roundtrip_320x240,
    bench_roundtrip_159x33,
    bench_roundtrip_1024x1024,
);
criterion_main!(benches);
