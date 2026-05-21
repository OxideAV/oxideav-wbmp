//! Decoder resource limits.
//!
//! WAP-237 doesn't normatively cap WBMP dimensions — the spec just says
//! "the device's display capability." That's fine on a real-world WAP
//! phone where bitmaps top out at the screen size (typically ≤ 320×240
//! on 2000-era handsets, ≤ a few thousand pixels even on modern OLED
//! handsets) but it leaves a sharp edge on a general-purpose decoder:
//! an attacker-crafted WBMP header carrying width = `u32::MAX` and
//! height = `u32::MAX` would, in principle, ask us to allocate
//! 2³⁰ × 2³⁰ ÷ 8 ≈ 1.4 × 10¹⁷ bytes of pixel buffer. Without an
//! explicit cap we'd either OOM the host or allocate gigabytes from a
//! 12-byte input.
//!
//! [`WbmpLimits`] is the spec-compatible safeguard: the standalone
//! [`crate::parse_wbmp`] uses defaults conservative enough to admit
//! every WBMP file ever shipped in practice (max dimension 16384,
//! max packed pixel-data 8 MiB) while bounding worst-case memory.
//! Callers that genuinely need larger images can opt in via
//! [`crate::parse_wbmp_with_limits`] with a customised
//! [`WbmpLimits`].

/// Decoder resource limits applied during header validation.
///
/// All limits are **inclusive** — `max_width = 16384` accepts a
/// 16384-pixel-wide bitmap and rejects one of 16385.
///
/// The defaults ([`WbmpLimits::default`]):
///
/// | Field             | Default        | Why                                                |
/// |-------------------|----------------|----------------------------------------------------|
/// | `max_width`       | 16 384         | Covers every shipped WBMP profile + 4 K wallpapers |
/// | `max_height`      | 16 384         | Same                                               |
/// | `max_pixel_bytes` | 8 388 608 (8 M)| Hard ceiling on the buffer the decoder allocates   |
///
/// At the defaults a malicious header asking for a 16384 × 16384 image
/// hits the pixel-byte cap (16384 × 16384 / 8 = 32 M) and is rejected
/// before the decoder touches its allocator. A real 1024 × 1024 image
/// only weighs 128 KiB so this leaves several orders of magnitude of
/// headroom for legitimate use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WbmpLimits {
    /// Maximum width in pixels (inclusive). 0 still illegal per
    /// [`crate::header::parse_header`].
    pub max_width: u32,
    /// Maximum height in pixels (inclusive).
    pub max_height: u32,
    /// Maximum packed pixel-data size in bytes (inclusive). The
    /// decoder pre-computes `stride * height` from the header MBIs and
    /// compares against this before allocating.
    pub max_pixel_bytes: usize,
}

impl Default for WbmpLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_pixel_bytes: 8 * 1024 * 1024,
        }
    }
}

impl WbmpLimits {
    /// Permissive limits suitable for trusted local input only —
    /// every dimension capped at `u32::MAX` and the pixel buffer
    /// capped at `usize::MAX`. Effectively disables the limit checks
    /// while still preserving the `u32` / `usize` overflow guards
    /// inside the decoder. **Do not** use on untrusted input.
    pub fn unbounded() -> Self {
        Self {
            max_width: u32::MAX,
            max_height: u32::MAX,
            max_pixel_bytes: usize::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_admit_typical_screen_sizes() {
        let lim = WbmpLimits::default();
        // Smallest possible: 1×1.
        assert!(1 <= lim.max_width && 1 <= lim.max_height);
        // 320×240 WAP-era device.
        assert!(320 <= lim.max_width && 240 <= lim.max_height);
        // 1080p screenshot.
        assert!(1920 <= lim.max_width && 1080 <= lim.max_height);
        // Pixel-byte cap of 8 MiB accepts a 2048×2048 1-bit bitmap
        // (524 288 bytes) with headroom.
        let stride = 2048_usize.div_ceil(8);
        let bytes = stride * 2048;
        assert!(bytes <= lim.max_pixel_bytes);
    }

    #[test]
    fn unbounded_actually_unbounded() {
        let lim = WbmpLimits::unbounded();
        assert_eq!(lim.max_width, u32::MAX);
        assert_eq!(lim.max_height, u32::MAX);
        assert_eq!(lim.max_pixel_bytes, usize::MAX);
    }
}
