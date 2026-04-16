//! Fill the official IRS Form 4868 (Application for Automatic Extension)
//! PDF in pure Rust. The blank form is bundled at compile time as an asset
//! (downloaded from `https://www.irs.gov/pub/irs-pdf/f4868.pdf`, tax year
//! 2025). We mutate the AcroForm field /V values, set /NeedAppearances on
//! the /AcroForm dict so PDF readers regenerate the visual appearance, and
//! re-emit the document.
//!
//! Field map (verified empirically by probing the 2025 PDF):
//!
//! | Path (under `topmostSubform[0]`) | Purpose |
//! |---|---|
//! | `Page1[0].VoucherHeader[0].f1_1[0]` | Fiscal year *beginning* date |
//! | `Page1[0].VoucherHeader[0].f1_2[0]` | Fiscal year *ending* date |
//! | `Page1[0].VoucherHeader[0].f1_3[0]` | Fiscal year ending year (20__) |
//! | `Page1[0].PartI_ReadOrder[0].f1_4[0]` | Line 1: Name(s) |
//! | `Page1[0].PartI_ReadOrder[0].f1_5[0]` | Line 1: Street address |
//! | `Page1[0].PartI_ReadOrder[0].f1_6[0]` | Line 1: City |
//! | `Page1[0].PartI_ReadOrder[0].f1_7[0]` | Line 1: State (2-char) |
//! | `Page1[0].PartI_ReadOrder[0].f1_8[0]` | Line 1: ZIP |
//! | `Page1[0].PartI_ReadOrder[0].f1_9[0]` | Line 2: Your SSN |
//! | `Page1[0].PartI_ReadOrder[0].f1_10[0]` | Line 3: Spouse's SSN |
//! | `Page1[0].f1_11[0]` | Line 4: Estimate of total tax liability |
//! | `Page1[0].f1_12[0]` | Line 5: Total payments |
//! | `Page1[0].f1_13[0]` | Line 6: Balance due |
//! | `Page1[0].f1_14[0]` | Line 7: Amount you're paying |
//! | `Page1[0].c1_1[0]` | Line 8: Out of country (checkbox) |
//! | `Page1[0].c1_2[0]` | Line 9: 1040-NR no withheld wages (checkbox) |
//! | `Page3[0].Col4[0].f3_1[0]` | Page 3: Direct Pay confirmation # |

use lopdf::{Document, Object, ObjectId, StringFormat};

/// The blank IRS Form 4868 (2025) PDF, bundled at compile time.
const FORM_4868_BLANK: &[u8] = include_bytes!("../assets/f4868-2025.pdf");

/// Data needed to fill Form 4868. All fields are optional — missing values
/// leave the form blank for the user to complete by hand.
#[derive(Default, Debug, Clone)]
pub struct Form4868Data {
    // ── Part I: Identification ─────────────────────────────────────────
    /// Line 1: Full taxpayer name(s). For joint returns, "First Last & First Last".
    pub name: Option<String>,
    /// Line 1: Street address (e.g. "123 Main St, Apt 4").
    pub address: Option<String>,
    /// Line 1: City (e.g. "Olympia").
    pub city: Option<String>,
    /// Line 1: State (2-char USPS code, e.g. "WA").
    pub state: Option<String>,
    /// Line 1: ZIP code (5 or 9 digits).
    pub zip: Option<String>,
    /// Line 2: Your SSN, formatted "###-##-####".
    pub ssn: Option<String>,
    /// Line 3: Spouse's SSN (only filled when filing jointly).
    pub spouse_ssn: Option<String>,

    // ── Part II: Individual Income Tax ─────────────────────────────────
    /// Line 4: Estimate of total tax liability in display form ("32993.07").
    pub total_tax: Option<String>,
    /// Line 5: Total payments in display form.
    pub total_payments: Option<String>,
    /// Line 6: Balance due (line 4 - line 5) in display form.
    pub balance_due: Option<String>,
    /// Line 7: Amount paying with this extension in display form.
    pub amount_paying: Option<String>,

    // ── Checkboxes (lines 8 + 9) ───────────────────────────────────────
    /// Line 8: Out of country and a US citizen/resident.
    pub out_of_country: bool,
    /// Line 9: File Form 1040-NR with no withheld wages.
    pub is_1040nr_no_wages: bool,

    // ── Fiscal-year filers (rare) ──────────────────────────────────────
    /// "For tax year beginning" date (e.g. "07/01"). Calendar-year filers leave blank.
    pub fy_begin: Option<String>,
    /// "Ending" date (e.g. "06/30").
    pub fy_end: Option<String>,
    /// "20__" the year that the fiscal year ends in (e.g. "26").
    pub fy_end_year: Option<String>,

    // ── Page 3: confirmation tracking ──────────────────────────────────
    /// Direct Pay confirmation number, recorded on page 3 once payment is made.
    pub confirmation_number: Option<String>,
}

/// Map a canonical IRS field path → text value to insert.
fn text_for(path: &str, d: &Form4868Data) -> Option<String> {
    match path {
        "topmostSubform[0].Page1[0].VoucherHeader[0].f1_1[0]" => d.fy_begin.clone(),
        "topmostSubform[0].Page1[0].VoucherHeader[0].f1_2[0]" => d.fy_end.clone(),
        "topmostSubform[0].Page1[0].VoucherHeader[0].f1_3[0]" => d.fy_end_year.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_4[0]" => d.name.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_5[0]" => d.address.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_6[0]" => d.city.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_7[0]" => d.state.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_8[0]" => d.zip.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_9[0]" => d.ssn.clone(),
        "topmostSubform[0].Page1[0].PartI_ReadOrder[0].f1_10[0]" => d.spouse_ssn.clone(),
        "topmostSubform[0].Page1[0].f1_11[0]" => d.total_tax.clone(),
        "topmostSubform[0].Page1[0].f1_12[0]" => d.total_payments.clone(),
        "topmostSubform[0].Page1[0].f1_13[0]" => d.balance_due.clone(),
        "topmostSubform[0].Page1[0].f1_14[0]" => d.amount_paying.clone(),
        "topmostSubform[0].Page3[0].Col4[0].f3_1[0]" => d.confirmation_number.clone(),
        _ => None,
    }
}

fn checkbox_for(path: &str, d: &Form4868Data) -> Option<bool> {
    match path {
        "topmostSubform[0].Page1[0].c1_1[0]" => Some(d.out_of_country),
        "topmostSubform[0].Page1[0].c1_2[0]" => Some(d.is_1040nr_no_wages),
        _ => None,
    }
}

/// Decode a PDF text string. PDF spec: if it starts with the BOM `FE FF`,
/// the bytes after are UTF-16BE; otherwise treat as PDFDocEncoding (we
/// approximate with Latin-1, which round-trips ASCII fine).
fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let mut units = Vec::with_capacity((bytes.len() - 2) / 2);
        let mut i = 2;
        while i + 1 < bytes.len() {
            units.push(((bytes[i] as u16) << 8) | (bytes[i + 1] as u16));
            i += 2;
        }
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Encode a Rust string as a PDF UTF-16BE text string with the leading BOM.
fn encode_pdf_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + s.len() * 2);
    out.extend_from_slice(&[0xFE, 0xFF]);
    for u in s.encode_utf16() {
        out.push((u >> 8) as u8);
        out.push((u & 0xff) as u8);
    }
    out
}

/// Walk the parent chain of a field dict, building the dotted path
/// (e.g. `topmostSubform[0].Page1[0].f1_11[0]`).
fn full_field_name(doc: &Document, id: ObjectId) -> Option<String> {
    let dict = doc.get_object(id).ok()?.as_dict().ok()?;
    let bytes = if let Ok(Object::String(b, _)) = dict.get(b"T") { b } else { return None; };
    let mut name = decode_pdf_string(bytes);
    let mut parent_ref = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    while let Some(pid) = parent_ref {
        let p_dict = match doc.get_object(pid).ok().and_then(|o| o.as_dict().ok()) {
            Some(d) => d,
            None => break,
        };
        if let Ok(Object::String(pn, _)) = p_dict.get(b"T") {
            name = format!("{}.{}", decode_pdf_string(pn), name);
        }
        parent_ref = p_dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    Some(name)
}

/// Fill Form 4868 with the supplied data and return the resulting PDF bytes.
pub fn fill_form_4868(data: &Form4868Data) -> Result<Vec<u8>, String> {
    let mut doc = Document::load_mem(FORM_4868_BLANK).map_err(|e| format!("load: {e}"))?;

    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    let mut applied = 0usize;
    for id in ids {
        let path = match full_field_name(&doc, id) {
            Some(p) => p,
            None => continue,
        };
        if let Some(val) = text_for(&path, data) {
            if val.is_empty() { continue; }
            let dict = doc.get_object_mut(id).map_err(|e| e.to_string())?
                .as_dict_mut().map_err(|e| e.to_string())?;
            dict.set(b"V".to_vec(), Object::String(encode_pdf_string(&val), StringFormat::Literal));
            applied += 1;
        } else if let Some(checked) = checkbox_for(&path, data) {
            // Form 4868 checkboxes use the "1" export value when checked
            // (verified by probing). /Off is the universal unchecked state.
            // We set both /V (the value the form holds) and /AS (the
            // appearance state shown right now) so readers without
            // appearance regeneration also display the check.
            let dict = doc.get_object_mut(id).map_err(|e| e.to_string())?
                .as_dict_mut().map_err(|e| e.to_string())?;
            let v = if checked { "1" } else { "Off" };
            dict.set(b"V".to_vec(), Object::Name(v.as_bytes().to_vec()));
            dict.set(b"AS".to_vec(), Object::Name(v.as_bytes().to_vec()));
            applied += 1;
        }
    }

    // Make the reader regenerate appearance streams from the new /V values
    // — without this, Adobe Reader / Preview / Chrome show the field as
    // blank even though the value is stored.
    if let Ok(acro_ref) = doc.catalog().and_then(|c| c.get(b"AcroForm")).and_then(|a| a.as_reference()) {
        if let Some(obj) = doc.objects.get_mut(&acro_ref) {
            if let Ok(d) = obj.as_dict_mut() {
                d.set(b"NeedAppearances".to_vec(), Object::Boolean(true));
            }
        }
    }

    if applied == 0 {
        return Err("no Form 4868 fields matched — bundled template may be wrong version".into());
    }

    let mut buf: Vec<u8> = Vec::with_capacity(FORM_4868_BLANK.len() + 4096);
    doc.save_to(&mut buf).map_err(|e| format!("save: {e}"))?;
    Ok(buf)
}
