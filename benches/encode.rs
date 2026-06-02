//! Criterion benchmarks for the WBMP encoder hot paths.
//!
//! Round 173 (depth-mode benchmarks). Companion to `decode.rs` and
//! `roundtrip.rs`. Each scenario builds a packed 1-bit plane (or an 8-bit
//! grayscale buffer for the threshold helper) and iterates the encoder
//! over it.
//!
//!   - **encode_8x8_solid**: 8×8 minimum-interesting size — isolates
//!     per-call overhead (header write + tiny copy) from any per-byte
//!     bandwidth cost.
//!   - **encode_96x64_typical**: 96×64 WAP-era handset bitmap.
//!   - **encode_320x240_qvga**: 320×240 QVGA fixture; both dimensions
//!     drive a 2-byte width MBI.
//!   - **encode_159x33_odd_width**: 159×33 odd-width fixture exercising
//!     the per-row padding-bit boundary the encoder writes (we already
//!     mask the padding bits in the source plane so the input bytes are
//!     canonical).
//!   - **encode_1024x1024_padded**: 1024×1024 mid-size fixture — checks
//!     bandwidth scaling against the smaller cases.
//!   - **encode_threshold_320x240_gray8**: 320×240 grayscale →
//!     1-bit threshold path via `encode_wbmp_from_threshold`. This
//!     exercises the per-pixel branch-and-set hot loop, which is the
//!     only non-trivial work the encoder does.
//!   - **encode_dither_320x240_gray8**: 320×240 grayscale → 1-bit
//!     dither path via `encode_wbmp_from_dither`. Exercises the
//!     stateful Floyd–Steinberg accumulator + per-row cur/next swap;
//!     a useful A/B against the threshold scenario to track the
//!     dither path's per-pixel cost regression-budget separately
//!     from the branch-and-set hot loop.
//!
//! Run with:
//!     cargo bench -p oxideav-wbmp --bench encode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_wbmp::{encode_wbmp, encode_wbmp_from_dither, encode_wbmp_from_threshold, WbmpImage};

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

fn build_gray8(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let n = (width as usize) * (height as usize);
    let mut data = vec![0u8; n];
    let mut state = seed;
    for byte in data.iter_mut() {
        *byte = xorshift_byte(&mut state);
    }
    data
}

fn bench_encode_8x8_solid(c: &mut Criterion) {
    let bits = build_packed_plane(8, 8, 0x1234_5678);
    let mut g = c.benchmark_group("encode_8x8_solid");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/8x8"), |b| {
        b.iter(|| encode_wbmp(8, 8, criterion::black_box(&bits)).expect("encode"));
    });
    g.finish();
}

fn bench_encode_96x64_typical(c: &mut Criterion) {
    let bits = build_packed_plane(96, 64, 0x2345_6789);
    let mut g = c.benchmark_group("encode_96x64_typical");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/96x64"), |b| {
        b.iter(|| encode_wbmp(96, 64, criterion::black_box(&bits)).expect("encode"));
    });
    g.finish();
}

fn bench_encode_320x240_qvga(c: &mut Criterion) {
    let bits = build_packed_plane(320, 240, 0x3456_789a);
    let mut g = c.benchmark_group("encode_320x240_qvga");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/320x240"), |b| {
        b.iter(|| encode_wbmp(320, 240, criterion::black_box(&bits)).expect("encode"));
    });
    g.finish();
}

fn bench_encode_159x33_odd_width(c: &mut Criterion) {
    let bits = build_packed_plane(159, 33, 0x4567_89ab);
    let mut g = c.benchmark_group("encode_159x33_odd_width");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("wbmp/159x33"), |b| {
        b.iter(|| encode_wbmp(159, 33, criterion::black_box(&bits)).expect("encode"));
    });
    g.finish();
}

fn bench_encode_1024x1024_padded(c: &mut Criterion) {
    let bits = build_packed_plane(1024, 1024, 0x5678_9abc);
    let mut g = c.benchmark_group("encode_1024x1024_padded");
    g.throughput(Throughput::Bytes(bits.len() as u64));
    g.sample_size(40);
    g.bench_function(BenchmarkId::from_parameter("wbmp/1024x1024"), |b| {
        b.iter(|| encode_wbmp(1024, 1024, criterion::black_box(&bits)).expect("encode"));
    });
    g.finish();
}

fn bench_encode_threshold_320x240_gray8(c: &mut Criterion) {
    let gray = build_gray8(320, 240, 0x789a_bcde);
    let mut g = c.benchmark_group("encode_threshold_320x240_gray8");
    g.throughput(Throughput::Bytes(gray.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("threshold/320x240"), |b| {
        b.iter(|| {
            encode_wbmp_from_threshold(320, 240, criterion::black_box(&gray), 128)
                .expect("encode_from_threshold")
        });
    });
    g.finish();
}

fn bench_encode_dither_320x240_gray8(c: &mut Criterion) {
    let gray = build_gray8(320, 240, 0x89ab_cdef);
    let mut g = c.benchmark_group("encode_dither_320x240_gray8");
    g.throughput(Throughput::Bytes(gray.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("dither/320x240"), |b| {
        b.iter(|| {
            encode_wbmp_from_dither(320, 240, criterion::black_box(&gray))
                .expect("encode_from_dither")
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_encode_8x8_solid,
    bench_encode_96x64_typical,
    bench_encode_320x240_qvga,
    bench_encode_159x33_odd_width,
    bench_encode_1024x1024_padded,
    bench_encode_threshold_320x240_gray8,
    bench_encode_dither_320x240_gray8,
);
criterion_main!(benches);
