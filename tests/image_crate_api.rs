//! Contract-level round-trip tests for `oxideav-wbmp` (`IMAGE_CRATE_API`),
//! exercised through the public crate API as a downstream consumer
//! would use it. Every case builds a synthetic image (no fixtures on
//! disk), writes it with `encode` / `encode_gray8` / `encode_frames`,
//! reads it back with `decode` / `decode_all` and checks every byte of
//! the recovered plane matches the input bit-for-bit.
//!
//! The property test drives a deterministic xorshift generator over
//! random dimensions (odd widths included) and random bit planes.
//!
//! These run on the default-feature build (registry on); the
//! standalone-build CI job covers `--no-default-features --lib`.

use oxideav_wbmp::{
    decode, decode_all, decode_rgb8, decode_with, encode, encode_all, encode_frames, encode_gray8,
    info, probe, DecodeOptions, EncodeOptions, Error, Frame, PixelFormat, WbmpImage,
};

/// Tiny deterministic PRNG so the property test needs no dependency.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A canonical wire-polarity plane (padding bits zero) filled from
/// `rng`.
fn random_bits(rng: &mut XorShift, width: u32, height: u32) -> Vec<u8> {
    let stride = WbmpImage::row_stride(width);
    let pad = stride * 8 - width as usize;
    let mask = if pad == 0 { 0xFF } else { 0xFFu8 << pad };
    let mut bits = vec![0u8; stride * height as usize];
    for (y, row) in bits.chunks_exact_mut(stride).enumerate() {
        for b in row.iter_mut() {
            *b = rng.next() as u8;
        }
        row[stride - 1] &= mask;
        let _ = y;
    }
    bits
}

fn assert_roundtrip(width: u32, height: u32, bits: &[u8]) {
    let stride = WbmpImage::row_stride(width);
    assert_eq!(
        bits.len(),
        stride * height as usize,
        "test bug: bits length"
    );
    let img = WbmpImage::from_bits(width, height, bits.to_vec()).unwrap();
    let encoded = encode(&img, &EncodeOptions::default()).unwrap();
    assert_eq!(
        probe(&encoded),
        width <= oxideav_wbmp::PROBE_MAX_DIMENSION,
        "probe treats dimensions above PROBE_MAX_DIMENSION as noise"
    );
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded, img);
    assert_eq!(decoded.width(), width);
    assert_eq!(decoded.height(), height);
    assert_eq!(decoded.format(), PixelFormat::MonoBlack);
    assert_eq!(decoded.planes.len(), 1);
    assert_eq!(decoded.planes[0].stride, stride);
    assert_eq!(decoded.as_bytes().unwrap(), bits);
    let i = info(&encoded).unwrap();
    assert_eq!((i.width, i.height, i.frames), (width, height, 1));
}

#[test]
fn roundtrip_8x8_solid_white_and_black() {
    assert_roundtrip(8, 8, &[0xFFu8; 8]);
    assert_roundtrip(8, 8, &[0u8; 8]);
}

#[test]
fn roundtrip_64x64_diagonal() {
    let stride = 8usize;
    let mut bits = vec![0u8; stride * 64];
    for y in 0..64usize {
        bits[y * stride + y / 8] |= 1 << (7 - (y % 8));
    }
    assert_roundtrip(64, 64, &bits);
}

#[test]
fn roundtrip_padded_width_159x33() {
    let stride = WbmpImage::row_stride(159);
    let mut bits = vec![0u8; stride * 33];
    for y in 0..33usize {
        for x in 0..159usize {
            let on = if y % 2 == 0 { true } else { x < 80 };
            if on {
                bits[y * stride + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    assert_roundtrip(159, 33, &bits);
}

#[test]
fn roundtrip_mbi_lengths() {
    // 32×1: both MBIs one byte, the whole header is 4 bytes.
    assert_roundtrip(32, 1, &[0b1100_1010u8; 4]);
    // 200×100: two-byte width MBI, one-byte height MBI.
    let stride = WbmpImage::row_stride(200);
    assert_roundtrip(200, 100, &vec![0b1010_1010u8; stride * 100]);
    // 16385×1: three-byte width MBI.
    let stride = WbmpImage::row_stride(16385);
    assert_roundtrip(16385, 1, &vec![0u8; stride]);
}

#[test]
fn property_random_planes_round_trip_exactly() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..400 {
        let width = 1 + rng.below(37) as u32;
        let height = 1 + rng.below(19) as u32;
        let bits = random_bits(&mut rng, width, height);
        assert_roundtrip(width, height, &bits);

        // The inverse polarity survives too, and both describe the
        // same picture.
        let img = WbmpImage::from_bits(width, height, bits.clone()).unwrap();
        let inv = img.clone().into_format(PixelFormat::MonoWhite);
        let encoded = encode(&inv, &EncodeOptions::default()).unwrap();
        assert_eq!(&encoded[encoded.len() - bits.len()..], &bits[..]);
        let back = decode_with(
            &encoded,
            &DecodeOptions::default().with_format(PixelFormat::MonoWhite),
        )
        .unwrap();
        assert_eq!(back, inv, "{width}×{height} MonoWhite");
        assert_eq!(back.to_gray8(), img.to_gray8());

        // Dirty padding bits encode to the canonical plane.
        let mut dirty = bits.clone();
        let stride = WbmpImage::row_stride(width);
        let pad = stride * 8 - width as usize;
        for row in dirty.chunks_exact_mut(stride) {
            row[stride - 1] |= (rng.next() as u8) & !(0xFFu8 << pad);
        }
        let dirty_img = WbmpImage::from_bits(width, height, dirty).unwrap();
        let encoded = encode(&dirty_img, &EncodeOptions::default()).unwrap();
        assert_eq!(
            decode(&encoded).unwrap(),
            img,
            "{width}×{height} dirty padding"
        );
    }
}

#[test]
fn property_random_animations_round_trip_exactly() {
    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    for _ in 0..60 {
        let width = 1 + rng.below(23) as u32;
        let height = 1 + rng.below(9) as u32;
        let count = 1 + rng.below(16) as usize;
        let frames: Vec<WbmpImage> = (0..count)
            .map(|_| {
                WbmpImage::from_bits(width, height, random_bits(&mut rng, width, height)).unwrap()
            })
            .collect();
        let bytes = encode_frames(&frames, &EncodeOptions::default()).unwrap();
        assert_eq!(info(&bytes).unwrap().frames as usize, count);
        let back = decode_all(&bytes).unwrap();
        assert_eq!(back.len(), count);
        for (i, (fr, img)) in back.iter().zip(&frames).enumerate() {
            assert_eq!(&fr.image, img, "frame {i} of {count} ({width}×{height})");
            assert_eq!(fr.index as usize, i);
            assert!(fr.delay.is_none());
        }
        assert_eq!(decode(&bytes).unwrap(), frames[0]);
    }
}

#[test]
fn encode_all_mirrors_decode_all_and_encode_frames() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for count in [1usize, 2, 7, 16] {
        let (width, height) = (1 + rng.below(40) as u32, 1 + rng.below(12) as u32);
        let frames: Vec<Frame> = (0..count)
            .map(|i| {
                Frame::new(
                    WbmpImage::from_bits(width, height, random_bits(&mut rng, width, height))
                        .unwrap(),
                    i as u32,
                )
            })
            .collect();
        let bytes = encode_all(&frames, &EncodeOptions::default()).unwrap();
        let images: Vec<WbmpImage> = frames.iter().map(|f| f.image.clone()).collect();
        assert_eq!(
            bytes,
            encode_frames(&images, &EncodeOptions::default()).unwrap()
        );
        if count == 1 {
            assert_eq!(
                bytes,
                encode(&images[0], &EncodeOptions::default()).unwrap()
            );
        }
        let back = decode_all(&bytes).unwrap();
        assert_eq!(back, frames, "{count} frames ({width}×{height})");
        // Frames straight out of `decode_all` re-encode byte-identically.
        assert_eq!(encode_all(&back, &EncodeOptions::default()).unwrap(), bytes);
    }
    // Empty, over-long and mismatched inputs are `InvalidData`.
    let one = |w: u32, h: u32| {
        Frame::new(
            WbmpImage::from_bits(w, h, vec![0; (w as usize).div_ceil(8) * h as usize]).unwrap(),
            0,
        )
    };
    assert!(matches!(
        encode_all(&[], &EncodeOptions::default()),
        Err(Error::InvalidData(_))
    ));
    let too_many: Vec<Frame> = (0..17).map(|_| one(3, 2)).collect();
    assert!(matches!(
        encode_all(&too_many, &EncodeOptions::default()),
        Err(Error::InvalidData(_))
    ));
    assert!(matches!(
        encode_all(&[one(3, 2), one(2, 3)], &EncodeOptions::default()),
        Err(Error::InvalidData(_))
    ));
}

#[test]
fn gray8_threshold_full_ramp() {
    // 256-pixel-wide ramp 0..=255 at threshold 128: 16 bytes 0x00
    // then 16 bytes 0xFF.
    let gray: Vec<u8> = (0u32..256).map(|v| v as u8).collect();
    let encoded = encode_gray8(256, 1, &gray, &EncodeOptions::default()).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.planes[0].stride, 32);
    let mut expected = [0u8; 32];
    expected[16..].fill(0xFF);
    assert_eq!(decoded.as_bytes().unwrap(), &expected[..]);
    // And back to 8-bit: 0 / 255 per channel.
    let rgb = decode_rgb8(&encoded).unwrap();
    assert_eq!(&rgb.data[..3], &[0, 0, 0]);
    assert_eq!(&rgb.data[rgb.data.len() - 3..], &[255, 255, 255]);
}

#[test]
fn gray8_2d_pattern_and_tail_bits() {
    let (w, h) = (24usize, 16usize);
    let mut gray = vec![0u8; w * h];
    let mut expected = vec![0u8; 3 * h];
    for y in 0..h {
        for x in 0..w {
            let bright = (y < h / 2) ^ (x < w / 4);
            gray[y * w + x] = if bright { 255 } else { 0 };
            if bright {
                expected[y * 3 + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    let encoded = encode_gray8(w as u32, h as u32, &gray, &EncodeOptions::default()).unwrap();
    assert_eq!(decode(&encoded).unwrap().as_bytes().unwrap(), &expected[..]);

    // Width 11: one full byte + a 3-bit tail, padding bits zero.
    let gray = [255u8, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255];
    let encoded = encode_gray8(11, 1, &gray, &EncodeOptions::default()).unwrap();
    assert_eq!(decode(&encoded).unwrap().as_bytes().unwrap(), &[0xAA, 0xA0]);
}

#[test]
fn encoded_byte_count_matches_handcalc() {
    let one = |w, h, n| {
        encode(
            &WbmpImage::from_bits(w, h, vec![0u8; n]).unwrap(),
            &EncodeOptions::default(),
        )
        .unwrap()
        .len()
    };
    assert_eq!(one(8, 8, 8), 12);
    assert_eq!(one(1, 1, 1), 5);
    assert_eq!(one(200, 1, 25), 30);
}

#[test]
fn limits_are_configurable_not_hard_coded() {
    let stride = WbmpImage::row_stride(16385);
    let encoded = encode(
        &WbmpImage::from_bits(16385, 1, vec![0u8; stride]).unwrap(),
        &EncodeOptions::default(),
    )
    .unwrap();
    assert!(
        decode(&encoded).is_ok(),
        "defaults cap bytes (1 GiB), not width"
    );
    assert!(matches!(
        decode_with(&encoded, &DecodeOptions::default().with_max_width(16384u32)),
        Err(Error::LimitExceeded(_))
    ));
    assert!(matches!(
        decode_with(&encoded, &DecodeOptions::default().with_max_bytes(2048u64)),
        Err(Error::LimitExceeded(_))
    ));
    assert_eq!(
        decode_with(&encoded, &DecodeOptions::default().unlimited())
            .unwrap()
            .width(),
        16385
    );
}
