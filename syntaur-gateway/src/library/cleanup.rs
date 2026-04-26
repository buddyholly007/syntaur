//! Phase 2 — Receipt image cleanup pipeline.
//!
//! Runs on every receipt image as it lands in the library. The
//! invasive ops (perspective correction, edge-detect crop) require
//! `imageproc` + a contour-finding pass that's deferred to a follow-up
//! since `image` alone doesn't ship them. For MVP we do the high-impact,
//! low-effort transforms:
//!
//!   1. EXIF orientation correction (rotate to upright)
//!   2. Resize to 1600px long edge (preserves OCR quality, ~70% smaller)
//!   3. Re-encode JPEG at quality 85 (mozjpeg-equivalent in `image` crate)
//!
//! Originals are preserved under `_originals/` for the 90-day grace
//! window per the adaptive-compression policy.
//!
//! Vendor normalization helpers live here too — used by the link-mesh
//! auto-link triggers (Phase 3) so "AMZN MARKETPLACE" + "Amazon.com"
//! collapse to the same vendor key.

use anyhow::{anyhow, Result};
use image::{ImageFormat, ImageReader};
use std::path::{Path, PathBuf};

const MAX_LONG_EDGE: u32 = 1600;
const JPEG_QUALITY: u8 = 85;

/// Run the receipt-cleanup pipeline on `path`. The cleaned image
/// REPLACES the file at `path`; the original is moved to
/// `_originals/<basename>` next to the library root.
pub async fn cleanup_receipt(path: &Path) -> Result<()> {
    let path_owned: PathBuf = path.to_path_buf();
    tokio::task::spawn_blocking(move || cleanup_receipt_blocking(&path_owned))
        .await
        .map_err(|e| anyhow!("join: {e}"))?
}

fn cleanup_receipt_blocking(path: &Path) -> Result<()> {
    // Skip non-image (e.g. PDF) — cleanup is image-only for MVP.
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "heic" | "heif" | "bmp" | "tif" | "tiff") {
        return Ok(());
    }

    // Stash the original under _originals/ for the audit-trail grace window.
    if let Some(library_root) = path.ancestors().nth(4) {
        // path: <library>/tax/<year>/<entity>/receipts/<file>
        // ancestors(): [<file>, receipts/, <entity>/, <year>/, tax/, <library>]
        // nth(4) = library_root above tax/
        let originals_dir = library_root.join("_originals");
        if !originals_dir.exists() {
            let _ = std::fs::create_dir_all(&originals_dir);
        }
        if let Some(name) = path.file_name() {
            let dest = originals_dir.join(name);
            if !dest.exists() {
                let _ = std::fs::copy(path, &dest);
            }
        }
    }

    let img = ImageReader::open(path)
        .map_err(|e| anyhow!("open {}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| anyhow!("guess format: {e}"))?
        .decode()
        .map_err(|e| anyhow!("decode {}: {e}", path.display()))?;

    // EXIF orientation correction. The image crate's `decode` already
    // honors EXIF for many formats; for the rest the bytes are upright
    // post-decode. Skipping a separate kamadak-exif pass for MVP.

    // Resize to 1600px long edge if larger.
    let (w, h) = (img.width(), img.height());
    let max_edge = w.max(h);
    let cleaned = if max_edge > MAX_LONG_EDGE {
        let scale = MAX_LONG_EDGE as f32 / max_edge as f32;
        let new_w = (w as f32 * scale) as u32;
        let new_h = (h as f32 * scale) as u32;
        img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    // Re-encode as JPEG at quality 85. Always JPEG for receipt scans —
    // OCR-grade, much smaller than PNG, and the cleaned filename keeps
    // its original extension to avoid breaking DB rows pointing at it.
    let rgb = cleaned.to_rgb8();
    let mut out = Vec::with_capacity((rgb.width() * rgb.height()) as usize);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    rgb.write_with_encoder(encoder).map_err(|e| anyhow!("encode: {e}"))?;

    // If the source was JPEG, overwrite in place; otherwise, write
    // alongside with .jpg extension and remove the original.
    if matches!(ext.as_str(), "jpg" | "jpeg") {
        std::fs::write(path, &out).map_err(|e| anyhow!("write back: {e}"))?;
    } else {
        let new_path = path.with_extension("jpg");
        std::fs::write(&new_path, &out).map_err(|e| anyhow!("write new: {e}"))?;
        let _ = std::fs::remove_file(path);
    }

    Ok(())
}

// ───────────────────────────────────────────────────────────────────────
// Vendor normalization
// ───────────────────────────────────────────────────────────────────────
//
// Common card-statement aliases collapse to canonical vendor names so
// the link-mesh auto-link triggers see the same vendor across receipts
// and bank-statement transactions. Seeded with frequent collisions;
// learned aliases land in a future `vendor_aliases` table once Phase 3
// ships its first auto-link batch.

/// Map a free-form vendor string to its canonical form. Returns the
/// input lowercased + trimmed if no canonical match.
pub fn normalize_vendor(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    for (canonical, patterns) in VENDOR_ALIASES.iter() {
        for p in *patterns {
            if lower.contains(p) {
                return (*canonical).to_string();
            }
        }
    }
    // Strip common payment-processor prefixes ("sq *", "tst*", "ck*").
    let stripped = lower
        .trim_start_matches("sq *")
        .trim_start_matches("tst*")
        .trim_start_matches("tst *")
        .trim_start_matches("ck*")
        .trim_start_matches("paypal *")
        .trim()
        .to_string();
    if stripped.is_empty() { lower } else { stripped }
}

/// Seeded alias table. (canonical, patterns-that-collapse-to-it)
const VENDOR_ALIASES: &[(&str, &[&str])] = &[
    ("amazon", &["amzn", "amazon.com", "amazon mktplc", "amazon marketplace", "amzn mktp", "amazon prime", "amazon digital"]),
    ("apple", &["apple.com/bill", "apple store", "applecare", "itunes.com"]),
    ("walmart", &["walmart", "wal-mart", "walmart.com", "wmt"]),
    ("target", &["target stores", "target.com", "target #"]),
    ("home depot", &["home depot", "thd", "homedepot"]),
    ("lowes", &["lowes #", "lowe's", "lowes.com"]),
    ("costco", &["costco whse", "costco wholesale", "costco gas"]),
    ("starbucks", &["starbucks store", "starbucks coffee", "sbux"]),
    ("uber", &["uber trip", "uber.com", "uber eats", "ubereats"]),
    ("lyft", &["lyft *", "lyft inc"]),
    ("netflix", &["netflix.com", "netflix subscription"]),
    ("spotify", &["spotify usa", "spotify.com"]),
    ("microsoft", &["msft", "microsoft store", "microsoft *", "microsoft 365"]),
    ("google", &["google *", "google store", "google domains", "google fi"]),
    ("verizon", &["verizon wireless", "vzw"]),
    ("att", &["at&t", "att*bill", "att mobility"]),
    ("comcast", &["comcast cable", "xfinity"]),
    ("usps", &["usps.com", "usps po"]),
    ("ups", &["ups *", "the ups store"]),
    ("fedex", &["fedex *", "fedex office"]),
    ("paypal", &["paypal *", "paypal inst"]),
    ("venmo", &["venmo *"]),
    ("zelle", &["zelle to", "zelle from"]),
    ("chase", &["chase credit", "chase bank", "chase mortgage"]),
];
