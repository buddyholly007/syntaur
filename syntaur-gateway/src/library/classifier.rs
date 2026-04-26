//! Vision-LLM classifier for library intake.
//!
//! Returns a `Classification` with kind + confidence + extracted metadata
//! (vendor, doc_date, year, entity, form_type). Routes through the same
//! provider chain as `tax::scan_receipt_vision`: configured "vision"
//! provider → local GPU swap → cloud fallback.
//!
//! Phase 1: fires the LLM and parses its structured JSON response.
//! Phase 2 will add image preprocessing (rotate / crop / resize) BEFORE
//! the classifier so we don't pay for the LLM on a 12MB raw camera frame.

use anyhow::{anyhow, Result};
use base64::Engine;
use std::sync::Arc;

use crate::library::Classification;
use crate::AppState;

const CLASSIFIER_PROMPT: &str = r#"You are a document classification system. Examine the attached image and classify it into EXACTLY ONE of these categories:

- **photo**: a personal photograph of people, pets, places, food, events. NOT a document.
- **receipt**: a store / restaurant / online-purchase receipt or invoice. Has vendor name + amount + date + line items.
- **tax_form**: an official tax form (W-2, 1099-NEC, 1099-INT, 1099-DIV, 1098, K-1, Schedule C/E, etc.) issued by an employer / payer / brokerage / lender.
- **personal_doc**: an identity document, certificate, deed, contract, will, medical record, insurance policy, license. NOT tax-related.
- **manual**: an owner's manual, user guide, warranty doc for a product.
- **unknown**: doesn't fit any of the above OR you can't tell.

Also extract:
- **doc_date**: any date printed on the document (YYYY-MM-DD); null if none.
- **vendor**: store name (for receipts), issuer (for tax forms), product manufacturer (for manuals); null otherwise.
- **form_type**: for tax_form only — exact form code (e.g. "W-2", "1099-NEC"); null otherwise.
- **entity**: for receipt and tax_form — "personal" or "business"; default "personal" if ambiguous.
- **year**: the tax year for tax_form (the year on the form); the year of the document otherwise; null if you can't tell.
- **alternatives**: up to 2 other categories you considered, with confidence each.
- **confidence**: your overall confidence in the chosen kind, 0.0 to 1.0.
- **notes**: 1-2 sentence rationale.

Respond with ONLY a JSON object of this exact shape (no prose, no code fences):
{
  "kind": "...",
  "confidence": 0.0,
  "doc_date": null,
  "vendor": null,
  "form_type": null,
  "entity": null,
  "year": null,
  "alternatives": [{"kind":"...","confidence":0.0}],
  "notes": "..."
}"#;

const LOCAL_GPU_HOST: &str = "192.168.1.69";
const LOCAL_VISION_PORT: u16 = 8900;

pub async fn classify(
    state: &Arc<AppState>,
    bytes: &[u8],
    content_type: &str,
    filename: &str,
) -> Result<Classification> {
    // PDFs need conversion → first-page PNG so vision models can read them.
    let (image_bytes, mime) = if content_type == "application/pdf" || filename.to_lowercase().ends_with(".pdf") {
        match convert_pdf_first_page(bytes) {
            Ok(png) => (png, "image/png".to_string()),
            Err(e) => {
                log::warn!("[library/classifier] PDF→PNG failed ({e}); using raw bytes");
                (bytes.to_vec(), content_type.to_string())
            }
        }
    } else {
        (bytes.to_vec(), content_type.to_string())
    };

    // Photos shortcut: extension-based fast path for known photo formats.
    // Vision-LLM call still runs to extract date / faces in Phase 4, but
    // for non-image / non-pdf payloads we know it's not a photo without
    // burning an LLM call.
    let lower = filename.to_lowercase();
    let is_image = mime.starts_with("image/")
        || lower.ends_with(".jpg") || lower.ends_with(".jpeg")
        || lower.ends_with(".png") || lower.ends_with(".webp")
        || lower.ends_with(".heic") || lower.ends_with(".heif");
    let is_pdf_origin = content_type == "application/pdf" || filename.to_lowercase().ends_with(".pdf");

    if !is_image && !is_pdf_origin {
        // Plain-text / data files: heuristic fallback. Default to
        // personal_doc, low confidence so it lands in the inbox for
        // human triage.
        return Ok(Classification {
            kind: "personal_doc".into(),
            confidence: 0.4,
            alternatives: vec![("unknown".into(), 0.3)],
            notes: Some(format!("non-image upload ({mime}); defaulted, please triage")),
            ..Classification::unknown()
        });
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

    let (url, model, api_key) = pick_vision_endpoint(state).await;
    if url.is_empty() {
        return Err(anyhow!("no vision provider configured"));
    }

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": CLASSIFIER_PROMPT},
                    {
                        "type": "image_url",
                        "image_url": {"url": format!("data:{mime};base64,{b64}")}
                    }
                ]
            }
        ],
        "temperature": 0.1,
        "max_tokens": 600
    });

    let req = state.client.post(&url).json(&body).timeout(std::time::Duration::from_secs(45));
    let req = if api_key.is_empty() { req } else { req.bearer_auth(&api_key) };
    let resp = req.send().await.map_err(|e| anyhow!("vision request: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        return Err(anyhow!("vision HTTP {status}: {}", &txt.chars().take(400).collect::<String>()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| anyhow!("vision json parse: {e}"))?;
    let text = v.get("choices").and_then(|c| c.get(0))
        .and_then(|c| c.get("message")).and_then(|m| m.get("content"))
        .and_then(|c| c.as_str()).unwrap_or("");

    // Some providers wrap the JSON in code fences; strip them.
    let cleaned = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    let parsed: serde_json::Value = serde_json::from_str(cleaned)
        .map_err(|e| anyhow!("classifier JSON parse: {e}; raw: {}", &cleaned.chars().take(300).collect::<String>()))?;

    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
    let confidence = parsed.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 1.0);
    let doc_date = parsed.get("doc_date").and_then(|v| v.as_str()).map(|s| s.to_string());
    let vendor = parsed.get("vendor").and_then(|v| v.as_str()).map(|s| s.to_string());
    let form_type = parsed.get("form_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    let entity = parsed.get("entity").and_then(|v| v.as_str()).map(|s| s.to_string());
    let year = parsed.get("year").and_then(|v| v.as_i64()).map(|n| n as i32);
    let notes = parsed.get("notes").and_then(|v| v.as_str()).map(|s| s.to_string());
    let alternatives: Vec<(String, f64)> = parsed.get("alternatives").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|a| {
            let k = a.get("kind").and_then(|v| v.as_str())?.to_string();
            let c = a.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            Some((k, c))
        }).collect())
        .unwrap_or_default();

    Ok(Classification {
        kind, confidence, alternatives, doc_date, notes, vendor, form_type, entity, year,
    })
}

/// Pick the best vision endpoint. Mirrors the priority chain in
/// tax::scan_receipt_vision: configured "vision" provider → local GPU
/// hot-swap on gaming-pc:8900 → cloud fallback (openrouter / anthropic /
/// openai whichever is configured).
async fn pick_vision_endpoint(state: &Arc<AppState>) -> (String, String, String) {
    let cfg = &state.config;

    // 1. Explicitly configured "vision" provider wins.
    if let Some(vp) = cfg.models.providers.get("vision") {
        let url = format!("{}/chat/completions", vp.base_url.trim_end_matches('/'));
        let model = vp.models.first().map(|m| m.id.clone()).unwrap_or_else(|| "qwen2.5-vl".into());
        return (url, model, vp.api_key.clone());
    }

    // 2. Try local GPU hot-swap (best-effort, fast timeout).
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new("ssh")
            .args([&format!("sean@{LOCAL_GPU_HOST}"), "bash", "/home/sean/swap-to-vision.sh"])
            .output(),
    )
    .await;
    if let Ok(Ok(out)) = probe {
        if String::from_utf8_lossy(&out.stdout).contains("READY") {
            return (
                format!("http://{LOCAL_GPU_HOST}:{LOCAL_VISION_PORT}/v1/chat/completions"),
                "qwen2.5-vl-7b".into(),
                String::new(),
            );
        }
    }

    // 3. Cloud fallback — first provider with vision-capable model.
    for (name, p) in cfg.models.providers.iter() {
        if name == "vision" { continue; }
        let m = if p.base_url.contains("openrouter") {
            "nvidia/nemotron-nano-12b-v2-vl:free"
        } else if p.base_url.contains("anthropic") {
            "claude-sonnet-4-6"
        } else if p.base_url.contains("openai") {
            "gpt-4o-mini"
        } else {
            continue;
        };
        return (
            format!("{}/chat/completions", p.base_url.trim_end_matches('/')),
            m.into(),
            p.api_key.clone(),
        );
    }
    (String::new(), String::new(), String::new())
}

fn convert_pdf_first_page(pdf_bytes: &[u8]) -> Result<Vec<u8>> {
    // Reuse the existing helper from the tax module which already shells
    // out to pdftoppm. Local helper to avoid leaking internal types.
    let tmp = tempfile::NamedTempFile::new().map_err(|e| anyhow!("tmpfile: {e}"))?;
    std::fs::write(tmp.path(), pdf_bytes).map_err(|e| anyhow!("write pdf: {e}"))?;
    let out_prefix = tmp.path().with_extension("page");
    let status = std::process::Command::new("pdftoppm")
        .args([
            "-png",
            "-r", "150",
            "-f", "1", "-l", "1",
            tmp.path().to_str().unwrap_or(""),
            out_prefix.to_str().unwrap_or(""),
        ])
        .status()
        .map_err(|e| anyhow!("pdftoppm: {e}"))?;
    if !status.success() {
        return Err(anyhow!("pdftoppm exited {status}"));
    }
    // pdftoppm names the output <prefix>-1.png (or .png if -singlefile).
    let candidates = [
        format!("{}-1.png", out_prefix.display()),
        format!("{}.png", out_prefix.display()),
        format!("{}-01.png", out_prefix.display()),
    ];
    for c in &candidates {
        if let Ok(b) = std::fs::read(c) {
            let _ = std::fs::remove_file(c);
            return Ok(b);
        }
    }
    Err(anyhow!("pdftoppm produced no output"))
}
