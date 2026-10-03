#![no_main]

//! Drive arbitrary fuzz-supplied bytes through the whole contract read
//! side (`IMAGE_CRATE_API`): `probe`, `info`, `decode`, `decode_with`
//! (strict + inverse polarity), `decode_all`, `decode_rgb8`,
//! `decode_rgba8`.
//!
//! Every call must *return* — malformed input yields `Err`, well-formed
//! input `Ok` — and neither path may panic, integer-overflow (debug
//! build), index out of bounds, or allocate an attacker-controlled
//! pixel buffer (the default `DecodeOptions` cap a plane at 1 GiB
//! before the allocator is touched). On top of "returns", the target
//! pins the cross-function invariants:
//!
//!  * `probe(b)` ⇒ `info(b)` succeeds with dimensions ≤ `PROBE_MAX_DIMENSION`.
//!  * `info` ⇒ `decode` succeeds iff the buffer holds the main image
//!    (header only, limit-free), and then `decode_all().len() ==
//!    info.frames`, every frame `stride × height` bytes, native
//!    `MonoBlack`, frame 0 == `decode`.
//!  * strict ⊆ lenient: a strict-accepted stream decodes identically
//!    through the lenient path.
//!  * `MonoWhite` request = bytewise inversion with zero padding bits,
//!    same `to_gray8()`.
//!  * `decode_rgb8` / `decode_rgba8` are the 0 / 255 expansion of the
//!    native plane.

use libfuzzer_sys::fuzz_target;
use oxideav_wbmp::{
    decode, decode_all, decode_rgb8, decode_rgba8, decode_with, info, probe, DecodeOptions,
    PixelFormat, PROBE_MAX_DIMENSION,
};

fuzz_target!(|data: &[u8]| {
    let probed = probe(data);
    let header = info(data);
    if probed {
        let h = header
            .as_ref()
            .expect("probe-accepted header must parse through info");
        assert!(h.width <= PROBE_MAX_DIMENSION && h.height <= PROBE_MAX_DIMENSION);
    }

    let lenient = decode(data);
    let strict = decode_with(data, &DecodeOptions::default().with_strict(true));
    if let Ok(simg) = &strict {
        let limg = lenient
            .as_ref()
            .expect("strict-accepted stream must decode leniently");
        assert_eq!(simg, limg, "strict ⊆ lenient, identical image");
    }

    let Ok(h) = header else {
        assert!(lenient.is_err(), "no header ⇒ no image");
        return;
    };
    assert_eq!(h.format, PixelFormat::MonoBlack);
    assert!(!h.has_alpha && !h.has_icc && !h.has_exif && !h.has_xmp);
    assert_eq!(
        (h.fix_header & 0x80) != 0,
        h.ext_fields.is_some(),
        "ExtFields presence tracks the FixHeaderField flag"
    );
    assert!(h.data_offset <= data.len());
    assert!((1..=16).contains(&h.frames));

    let stride = (h.width as usize).div_ceil(8);
    let Some(total) = stride.checked_mul(h.height as usize) else {
        assert!(lenient.is_err());
        return;
    };
    let fits = data.len() - h.data_offset >= total;
    if total as u64 > DecodeOptions::DEFAULT_MAX_BYTES {
        assert!(
            matches!(lenient, Err(oxideav_wbmp::Error::LimitExceeded(_))),
            "oversize plane trips the default cap before allocation"
        );
        return;
    }
    match &lenient {
        Ok(img) => {
            assert!(fits, "decode cannot succeed on a truncated body");
            assert_eq!((img.width, img.height), (h.width, h.height));
            assert_eq!(img.format(), PixelFormat::MonoBlack);
            assert_eq!(img.planes.len(), 1);
            assert_eq!(img.planes[0].stride, stride);
            assert_eq!(img.planes[0].data.len(), total);
            assert_eq!(
                &img.planes[0].data[..],
                &data[h.data_offset..h.data_offset + total],
                "native plane is the wire bytes verbatim"
            );

            // Every frame of the stream.
            let frames = decode_all(data).expect("decode ⇒ decode_all");
            assert_eq!(frames.len() as u32, h.frames);
            assert_eq!(&frames[0].image, img);
            for (i, fr) in frames.iter().enumerate() {
                assert_eq!(fr.index as usize, i);
                assert!(fr.delay.is_none());
                assert_eq!(fr.image.planes[0].data.len(), total);
                assert_eq!(fr.image.format(), PixelFormat::MonoBlack);
            }

            // Inverse polarity.
            let inv = decode_with(
                data,
                &DecodeOptions::default().with_format(PixelFormat::MonoWhite),
            )
            .expect("polarity request cannot fail where decode succeeds");
            assert_eq!(inv.format(), PixelFormat::MonoWhite);
            assert_eq!(inv.to_gray8(), img.to_gray8());
            let pad = stride * 8 - h.width as usize;
            let mask: u8 = if pad == 0 { 0xFF } else { 0xFFu8 << pad };
            for (row_inv, row) in inv.planes[0]
                .data
                .chunks_exact(stride)
                .zip(img.planes[0].data.chunks_exact(stride))
            {
                for (i, (a, b)) in row_inv.iter().zip(row).enumerate() {
                    let expect = if i + 1 == stride { !b & mask } else { !b };
                    assert_eq!(*a, expect, "inverted byte with zero padding");
                }
            }

            // Raw 8-bit paths.
            let gray = img.to_gray8();
            assert_eq!(gray.len(), h.width as usize * h.height as usize);
            assert!(gray.iter().all(|&g| g == 0 || g == 255));
            let rgb = decode_rgb8(data).expect("decode ⇒ decode_rgb8");
            assert_eq!(rgb.data.len(), gray.len() * 3);
            let rgba = decode_rgba8(data).expect("decode ⇒ decode_rgba8");
            assert_eq!(rgba.data.len(), gray.len() * 4);
            for ((g, p3), p4) in gray
                .iter()
                .zip(rgb.data.chunks_exact(3))
                .zip(rgba.data.chunks_exact(4))
            {
                assert_eq!(p3, &[*g, *g, *g]);
                assert_eq!(p4, &[*g, *g, *g, 255]);
            }
        }
        Err(_) => {
            assert!(!fits, "a complete body under the cap must decode");
            assert!(decode_all(data).is_err());
        }
    }
});
