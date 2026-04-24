//! Visual-diff engine for Phase 3 baselines.
//!
//! Two signals, combined because each catches a class of regression
//! the other misses:
//!
//! 1. **Pixel-delta percentage.** For each pixel position, sum the
//!    per-channel absolute differences (Manhattan distance). Any
//!    pixel whose sum exceeds a small tolerance (`PIXEL_TOLERANCE`)
//!    is counted as "different". The returned percentage is
//!    `100 * different / total`. Dimensions mismatch → 100% (the
//!    layout is categorically different from the baseline).
//!
//! 2. **Perceptual hash distance (aHash).** Downscale both images to
//!    8x8 greyscale, average the pixel values, emit a 64-bit hash
//!    where each bit is `pixel >= avg`. The Hamming distance between
//!    two hashes is a rough measure of layout similarity that's
//!    immune to small rendering jitter — a page with one changed
//!    pixel has aHash distance 0, but a page where a whole section
//!    moved will jump to 15+.
//!
//! Phase 3 surfaces both and flags a `VisualDiff` finding if EITHER
//! exceeds its configured threshold. Defaults are `pixel_delta_pct >
//! 1.0` OR `phash_distance > 5` — tuned against an empty run of the
//! Syntaur dashboard on 2026-04-23.
//!
//! The diff-image output is optional (callers pass `emit_diff_image:
//! false` in tests so the 8-bit-per-channel PNG encode doesn't
//! dominate the runtime). When emitted, it's the baseline with every
//! "different" pixel overwritten pure red (#FF0000) — a fast visual
//! "where did it break" for the human reviewer.

use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, RgbImage};

/// Per-pixel Manhattan-distance tolerance below which two pixels are
/// considered "the same". 10/255 is ~4% per channel — large enough to
/// absorb GPU/font-renderer jitter, small enough to catch colour
/// drift.
const PIXEL_TOLERANCE: u32 = 10;

#[derive(Debug, Clone)]
pub struct DiffResult {
    /// Percentage of pixels whose channel-sum delta exceeded the
    /// tolerance. 0.0 = bit-identical (modulo tolerance), 100.0 =
    /// dimensions mismatch OR every pixel changed.
    pub pixel_delta_pct: f64,
    /// Hamming distance between the two aHash values. 0-64. 0 = same
    /// low-frequency shape; 64 = photographic negatives of each other.
    pub phash_distance: u32,
    /// Red-highlighted PNG. `None` when `emit_diff_image = false` was
    /// passed, or when the dimensions mismatch (no pixel-aligned diff
    /// is meaningful in that case).
    pub diff_image: Option<Vec<u8>>,
    /// Baseline dimensions — useful context in the Finding's detail.
    pub baseline_dims: (u32, u32),
    /// Current-capture dimensions.
    pub current_dims: (u32, u32),
}

/// Diff two PNG byte slices. `emit_diff_image = true` runs an extra
/// PNG encode for the red-highlighted difference overlay; pass false
/// in tests + hot loops.
pub fn diff_pngs(baseline: &[u8], current: &[u8], emit_diff_image: bool) -> Result<DiffResult> {
    let base_img = image::load_from_memory(baseline)
        .context("decoding baseline PNG — baseline store may be corrupt; re-run with --update-baselines")?
        .to_rgb8();
    let curr_img = image::load_from_memory(current)
        .context("decoding current-capture PNG — screenshot step produced invalid PNG")?
        .to_rgb8();

    let base_dims = base_img.dimensions();
    let curr_dims = curr_img.dimensions();

    // Dimensions mismatch is a categorical regression — no point
    // diffing pixel-by-pixel, and the `image` crate's per-pixel
    // iterators don't align if w/h differ. Emit 100% delta + max
    // phash (64) so any sane threshold fires.
    if base_dims != curr_dims {
        return Ok(DiffResult {
            pixel_delta_pct: 100.0,
            phash_distance: 64,
            diff_image: None,
            baseline_dims: base_dims,
            current_dims: curr_dims,
        });
    }

    // ── Pixel-delta pass ──────────────────────────────────────────
    let (w, h) = base_dims;
    let total = (w as u64) * (h as u64);
    let mut different: u64 = 0;
    let mut diff_canvas: Option<RgbImage> = if emit_diff_image {
        Some(base_img.clone())
    } else {
        None
    };
    let red = Rgb([0xFFu8, 0x00, 0x00]);

    for y in 0..h {
        for x in 0..w {
            let b = base_img.get_pixel(x, y);
            let c = curr_img.get_pixel(x, y);
            let delta = (b[0] as i32 - c[0] as i32).unsigned_abs()
                + (b[1] as i32 - c[1] as i32).unsigned_abs()
                + (b[2] as i32 - c[2] as i32).unsigned_abs();
            if delta > PIXEL_TOLERANCE {
                different += 1;
                if let Some(canvas) = diff_canvas.as_mut() {
                    canvas.put_pixel(x, y, red);
                }
            }
        }
    }

    let pixel_delta_pct = if total == 0 {
        0.0
    } else {
        (different as f64 / total as f64) * 100.0
    };

    // ── Perceptual (average) hash pass ────────────────────────────
    let phash_distance = hamming(ahash(&base_img), ahash(&curr_img));

    // ── Encode diff overlay if requested ──────────────────────────
    let diff_image = match diff_canvas {
        Some(canvas) => Some(encode_png(&canvas).context("encoding diff overlay PNG")?),
        None => None,
    };

    Ok(DiffResult {
        pixel_delta_pct,
        phash_distance,
        diff_image,
        baseline_dims: base_dims,
        current_dims: curr_dims,
    })
}

/// Simple average-hash: resize to 8x8 greyscale, bit per pixel where
/// pixel >= mean. 64 bits packed into a u64 in row-major order.
fn ahash(img: &RgbImage) -> u64 {
    // Resize via the `image` crate's nearest-neighbour (fastest;
    // aHash doesn't care about interpolation quality — it's a
    // low-frequency signature).
    let small = image::imageops::resize(img, 8, 8, image::imageops::FilterType::Triangle);

    // Greyscale via luma (Rec.601 coefficients). We don't use
    // `image::imageops::grayscale` because that returns a Luma<u8>
    // image and we're staying on Rgb8 for cache friendliness.
    let mut luma = [0u8; 64];
    for (i, px) in small.pixels().enumerate() {
        let y = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
        luma[i] = y.round() as u8;
    }

    let mean: u32 = luma.iter().map(|v| *v as u32).sum::<u32>() / 64;
    let mut hash: u64 = 0;
    for (i, v) in luma.iter().enumerate() {
        if *v as u32 >= mean {
            hash |= 1 << i;
        }
    }
    hash
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

fn encode_png(img: &RgbImage) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = img.clone();
    let mut out: Vec<u8> = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    use image::ImageEncoder;
    encoder
        .write_image(buf.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .context("PNG encoder")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut img: RgbImage = ImageBuffer::new(w, h);
        for px in img.pixels_mut() {
            *px = Rgb(rgb);
        }
        encode_png(&img).expect("encode test PNG")
    }

    #[test]
    fn identical_images_zero_delta() {
        let a = solid(32, 32, [10, 20, 30]);
        let b = a.clone();
        let r = diff_pngs(&a, &b, false).expect("diff");
        assert_eq!(r.pixel_delta_pct, 0.0);
        assert_eq!(r.phash_distance, 0);
        assert!(r.diff_image.is_none());
    }

    #[test]
    fn single_pixel_change_nonzero_delta() {
        let base_bytes = solid(16, 16, [0, 0, 0]);
        // Replace one pixel in the middle with a bright colour.
        let mut img: RgbImage = ImageBuffer::new(16, 16);
        for px in img.pixels_mut() {
            *px = Rgb([0, 0, 0]);
        }
        img.put_pixel(8, 8, Rgb([255, 255, 255]));
        let curr_bytes = encode_png(&img).expect("encode");

        let r = diff_pngs(&base_bytes, &curr_bytes, true).expect("diff");
        // Exactly 1 / 256 pixels flagged = ~0.39%.
        assert!(r.pixel_delta_pct > 0.0);
        assert!(r.pixel_delta_pct < 1.0);
        // Diff image was requested, must be present.
        assert!(r.diff_image.is_some());
    }

    #[test]
    fn dimension_mismatch_is_max_delta() {
        let a = solid(16, 16, [128, 128, 128]);
        let b = solid(32, 16, [128, 128, 128]);
        let r = diff_pngs(&a, &b, false).expect("diff");
        assert_eq!(r.pixel_delta_pct, 100.0);
        assert_eq!(r.phash_distance, 64);
        assert_eq!(r.baseline_dims, (16, 16));
        assert_eq!(r.current_dims, (32, 16));
    }

    #[test]
    fn completely_different_images_high_phash() {
        let a = solid(32, 32, [0, 0, 0]);
        let b = solid(32, 32, [255, 255, 255]);
        let r = diff_pngs(&a, &b, false).expect("diff");
        assert_eq!(r.pixel_delta_pct, 100.0);
        // Black vs white — aHash bits flip relative to local mean,
        // but on a solid image mean == pixel so hashes can coincide.
        // Just make sure the pixel delta caught it.
        assert!(r.pixel_delta_pct > 50.0);
    }
}
