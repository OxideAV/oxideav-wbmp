//! Criterion benchmarks for the WBMP multi-frame (animation) path —
//! `encode_wbmp_frames` and `parse_wbmp_frames`.
//!
//! Round 333 (depth-mode benchmark). Companion to `decode.rs`,
//! `encode.rs`, and `roundtrip.rs`, which only exercise the single-image
//! entry points. WAP-237 §4.2 / §4.5.1 allow a main image to be followed
//! by 0..15 same-dimension animated sub-images; that sequence is decoded
//! by `parse_wbmp_frames` (a per-frame body-length check + verbatim plane
//! copy loop) and emitted by `encode_wbmp_frames` (a single shared header
//! followed by N back-to-back plane payloads). Those loops have no
//! benchmark coverage elsewhere — every other bench stops at one frame.
//!
//! Each iteration measures the combined encode → decode cost so the
//! per-frame fixed overhead (header re-walk on decode, frame-count bound
//! check) is amortised across the swept frame count, isolating the
//! marginal cost of each extra animated sub-image.
//!
//!   - **frames_96x64_x1**:  main image only (degenerate 1-frame case).
//!   - **frames_96x64_x4**:  main + 3 animated sub-images.
//!   - **frames_96x64_x16**: main + 15 animated sub-images (the spec max).
//!   - **frames_320x240_x8**: QVGA main + 7 animated sub-images.
//!
//! Run with:
//!     cargo bench -p oxideav-wbmp --bench frames

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_wbmp::{encode_wbmp_frames, parse_wbmp_frames, WbmpImage};

fn xorshift_byte(state: &mut u32) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state & 0xff) as u8
}

/// Build one packed `MonoWhite` plane with pseudo-random body bits and
/// the right padding bits in the last byte of each row zeroed, matching
/// what a well-formed encoder emits.
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

/// Build `frame_count` distinct packed planes (each seeded differently so
/// the optimiser can't fold them together).
fn build_frames(width: u32, height: u32, frame_count: usize) -> Vec<Vec<u8>> {
    (0..frame_count)
        .map(|i| {
            build_packed_plane(
                width,
                height,
                0x1357_0000u32.wrapping_add(i as u32 * 0x9e37),
            )
        })
        .collect()
}

fn run_frames_roundtrip(width: u32, height: u32, frames: &[Vec<u8>]) {
    let refs: Vec<&[u8]> = frames.iter().map(|f| f.as_slice()).collect();
    let encoded = encode_wbmp_frames(width, height, &refs).expect("encode frames");
    let decoded = parse_wbmp_frames(&encoded).expect("decode frames");
    criterion::black_box(decoded);
}

fn total_bytes(frames: &[Vec<u8>]) -> u64 {
    frames.iter().map(|f| f.len() as u64).sum()
}

fn bench_frames(c: &mut Criterion, name: &str, width: u32, height: u32, frame_count: usize) {
    let frames = build_frames(width, height, frame_count);
    let mut g = c.benchmark_group(name);
    g.throughput(Throughput::Bytes(total_bytes(&frames)));
    g.bench_function(
        BenchmarkId::from_parameter(format!("wbmp/{width}x{height}x{frame_count}")),
        |b| {
            b.iter(|| run_frames_roundtrip(width, height, criterion::black_box(&frames)));
        },
    );
    g.finish();
}

fn bench_frames_96x64_x1(c: &mut Criterion) {
    bench_frames(c, "frames_96x64_x1", 96, 64, 1);
}

fn bench_frames_96x64_x4(c: &mut Criterion) {
    bench_frames(c, "frames_96x64_x4", 96, 64, 4);
}

fn bench_frames_96x64_x16(c: &mut Criterion) {
    bench_frames(c, "frames_96x64_x16", 96, 64, 16);
}

fn bench_frames_320x240_x8(c: &mut Criterion) {
    bench_frames(c, "frames_320x240_x8", 320, 240, 8);
}

criterion_group!(
    benches,
    bench_frames_96x64_x1,
    bench_frames_96x64_x4,
    bench_frames_96x64_x16,
    bench_frames_320x240_x8,
);
criterion_main!(benches);
