//! Phase 3 — Cover-sheet PDF generator for tax-year audit exports.
//!
//! Hands a CPA everything they need at a glance: totals by category,
//! count of receipts/forms/statements, list of all docs with their
//! sha256 + doc date, and a note that the accompanying manifest.json
//! has the structured payload + (when enabled) the link mesh.

use anyhow::{anyhow, Result};
use printpdf::*;

use crate::library::year_archive::YearManifest;

pub fn build_year_cover(manifest: &YearManifest) -> Result<Vec<u8>> {
    let (doc, page1, layer1) = PdfDocument::new(
        format!("Tax Year {} — Audit Cover Sheet", manifest.year),
        Mm(210.0), Mm(297.0),
        "Page 1",
    );
    let font = doc.add_builtin_font(BuiltinFont::Helvetica).map_err(|e| anyhow!("font: {e}"))?;
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold).map_err(|e| anyhow!("font: {e}"))?;

    let layer = doc.get_page(page1).get_layer(layer1);

    // Title
    layer.use_text(format!("Tax Year {}", manifest.year), 24.0, Mm(20.0), Mm(275.0), &bold);
    layer.use_text("Audit Cover Sheet — Syntaur Library Export", 12.0, Mm(20.0), Mm(265.0), &font);

    let generated = chrono::DateTime::<chrono::Utc>::from_timestamp(manifest.generated_at, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "?".into());
    layer.use_text(format!("Generated: {generated}"), 10.0, Mm(20.0), Mm(258.0), &font);

    let mut y = 240.0;

    // Summary box
    layer.use_text("Summary", 14.0, Mm(20.0), Mm(y), &bold);
    y -= 8.0;
    layer.use_text(
        format!("Total files: {}", manifest.total_files),
        11.0, Mm(20.0), Mm(y), &font,
    );
    y -= 6.0;
    layer.use_text(
        format!("Total size: {:.2} MB", (manifest.total_size_bytes as f64) / 1_048_576.0),
        11.0, Mm(20.0), Mm(y), &font,
    );
    y -= 6.0;
    layer.use_text(
        format!("Link mesh: {}", if manifest.link_mesh_enabled { "ENABLED — see manifest.json for cross-references" } else { "disabled (simple-mode)" }),
        11.0, Mm(20.0), Mm(y), &font,
    );
    y -= 12.0;

    // Breakdown by kind
    layer.use_text("Breakdown by document kind", 14.0, Mm(20.0), Mm(y), &bold);
    y -= 8.0;
    for (kind, info) in manifest.by_kind.iter() {
        let count = info.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
        let size = info.get("size_bytes").and_then(|v| v.as_i64()).unwrap_or(0);
        layer.use_text(
            format!("  • {}: {} files ({:.2} KB)", kind, count, size as f64 / 1024.0),
            11.0, Mm(20.0), Mm(y), &font,
        );
        y -= 6.0;
    }
    y -= 8.0;

    // File list (truncated to fit)
    layer.use_text("File index", 14.0, Mm(20.0), Mm(y), &bold);
    y -= 6.0;
    layer.use_text("(See manifest.json for complete list with sha256.)", 9.0, Mm(20.0), Mm(y), &font);
    y -= 8.0;

    let mut current_layer = doc.get_page(page1).get_layer(layer1);
    let mut current_page_y = y;
    let max_files_first_page = 22;
    for (i, entry) in manifest.files.iter().enumerate() {
        if i == max_files_first_page {
            // Page 2
            let (page2, layer2) = doc.add_page(Mm(210.0), Mm(297.0), "Page 2");
            current_layer = doc.get_page(page2).get_layer(layer2);
            current_page_y = 280.0;
        }
        if current_page_y < 20.0 { break; } // out of page budget
        let date = entry.doc_date.as_deref().unwrap_or("—");
        let vendor = entry.vendor.as_deref().unwrap_or("—");
        let line = format!("{} | {} | {} | {}", date, entry.kind, vendor, &entry.relative_path);
        let truncated: String = line.chars().take(110).collect();
        current_layer.use_text(truncated, 8.5, Mm(20.0), Mm(current_page_y), &font);
        current_page_y -= 5.0;
    }

    let mut buf = Vec::new();
    doc.save(&mut std::io::BufWriter::new(&mut buf)).map_err(|e| anyhow!("pdf save: {e}"))?;
    Ok(buf)
}
