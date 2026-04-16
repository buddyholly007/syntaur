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

use lopdf::{dictionary, Document, Object, ObjectId, Stream, StringFormat};

/// The blank IRS Form 4868 (2025) PDF, bundled at compile time.
const FORM_4868_BLANK: &[u8] = include_bytes!("../assets/f4868-2025.pdf");

// ── IRS Where-To-File chart (Form 4868 page 4, 2025) ───────────────────────
// State groups taken verbatim from the official PDF. Every US state +
// territory routes to one of three IRS Service Centers (Austin / Kansas
// City / Ogden) when no payment is enclosed, or to one of two PO boxes
// (Charlotte NC / Louisville KY) when a check is enclosed.

/// Postal address ready to print on an envelope.
#[derive(Clone, Debug)]
pub struct IrsAddress {
    pub line1: &'static str,
    pub line2: &'static str,
    pub line3: &'static str,
}

impl IrsAddress {
    pub fn lines(&self) -> [&'static str; 3] { [self.line1, self.line2, self.line3] }
}

/// Resolve the correct IRS mailing address for Form 4868.
///
/// `state` is the 2-letter USPS code of the taxpayer's address.
/// `with_payment` is true when the filer is enclosing a check or money order.
pub fn irs_mailing_address(state: &str, with_payment: bool) -> IrsAddress {
    let st = state.trim().to_uppercase();
    let st = st.as_str();
    // Austin, TX group — Southeast + Southwest, no payment
    let austin_no_pay = ["AL", "FL", "GA", "LA", "MS", "NC", "SC", "TN", "TX",
                         "AZ", "AR", "NM", "OK"];
    // Kansas City, MO group — Northeast + Mid-Atlantic + Midwest, no payment
    let kc_no_pay = ["CT", "DE", "DC", "IL", "IN", "IA", "KY", "ME", "MD", "MA",
                     "MN", "MO", "NH", "NJ", "NY", "PA", "RI", "VT", "VA", "WV", "WI"];
    // Ogden, UT group — West + Mountain + remaining, no payment
    let ogden_no_pay = ["AK", "CA", "CO", "HI", "ID", "KS", "MI", "MT", "NE",
                        "NV", "ND", "OH", "OR", "SD", "UT", "WA", "WY"];
    // Charlotte NC P.O. Box for SE/SW with payment
    let charlotte_pay = ["AL", "FL", "GA", "LA", "MS", "NC", "SC", "TN", "TX"];

    if with_payment {
        if charlotte_pay.contains(&st) {
            IrsAddress { line1: "Internal Revenue Service",
                         line2: "P.O. Box 1302",
                         line3: "Charlotte, NC 28201-1302" }
        } else {
            // All other US states route their payment to Louisville KY
            IrsAddress { line1: "Internal Revenue Service",
                         line2: "P.O. Box 931300",
                         line3: "Louisville, KY 40293-1300" }
        }
    } else if austin_no_pay.contains(&st) {
        IrsAddress { line1: "Department of the Treasury",
                     line2: "Internal Revenue Service Center",
                     line3: "Austin, TX 73301-0045" }
    } else if kc_no_pay.contains(&st) {
        IrsAddress { line1: "Department of the Treasury",
                     line2: "Internal Revenue Service Center",
                     line3: "Kansas City, MO 64999-0045" }
    } else if ogden_no_pay.contains(&st) {
        IrsAddress { line1: "Department of the Treasury",
                     line2: "Internal Revenue Service Center",
                     line3: "Ogden, UT 84201-0045" }
    } else {
        // Foreign address / APO / Form 2555 / dual-status alien — page 4
        // routes these to Austin TX 73301-0215
        IrsAddress { line1: "Department of the Treasury",
                     line2: "Internal Revenue Service Center",
                     line3: "Austin, TX 73301-0215 USA" }
    }
}

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

// ── Cover page generator (mailing instructions + envelope guide) ─────────
// Prepends a US-Letter sized, printer-friendly first page to the form so
// the user can see exactly what to do, where to mail it, and what their
// own return address looks like in USPS-friendly format.
//
// We use lopdf primitives + the built-in Helvetica font (Standard 14)
// because that's the only way to keep the PDF self-contained without
// embedding font files. Helvetica + Helvetica-Bold are guaranteed to
// render identically in every PDF reader.

/// Escape a string for inclusion in a PDF literal-string `(…)` operator.
/// Backslash, paren, and non-ASCII bytes need handling. We strip non-ASCII
/// since the standard Helvetica encoding only covers WinAnsi.
fn pdf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 128 => out.push(c),
            // Map a couple of common non-ASCII glyphs back to ASCII so
            // accented names + smart quotes don't get dropped.
            'é' | 'è' | 'ê' => out.push('e'),
            'á' | 'à' | 'â' => out.push('a'),
            'í' | 'ì' | 'î' => out.push('i'),
            'ó' | 'ò' | 'ô' => out.push('o'),
            'ú' | 'ù' | 'û' => out.push('u'),
            'ñ' => out.push('n'),
            '—' | '–' => out.push('-'),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201C}' | '\u{201D}' => out.push('"'),
            // U+00B7 middle dot, U+2022 bullet — map to a printable ASCII
            // bullet so list markers don't turn into question marks.
            '·' | '•' => out.push('*'),
            // En-space, em-space, non-breaking space — standard space.
            '\u{00A0}' | '\u{2002}' | '\u{2003}' => out.push(' '),
            _ => out.push('?'),
        }
    }
    out
}

/// Build a PDF content stream for a single text-only Letter-size page. Each
/// entry in `lines` is `(font_id_str, size_pt, x_pt, y_pt_from_bottom, text)`.
fn build_text_content_stream(lines: &[(&str, f32, f32, f32, String)]) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("BT\n");
    let mut last_font: Option<(&str, f32)> = None;
    for (font, size, x, y, text) in lines {
        if last_font != Some((*font, *size)) {
            out.push_str(&format!("/{} {} Tf\n", font, size));
            last_font = Some((*font, *size));
        }
        // Set absolute text matrix: 1 0 0 1 x y Tm puts the cursor at
        // (x, y) regardless of where the previous text left off — no
        // arithmetic from the caller required.
        out.push_str(&format!("1 0 0 1 {} {} Tm\n", x, y));
        out.push_str(&format!("({}) Tj\n", pdf_escape(text)));
    }
    out.push_str("ET\n");
    // Draw two horizontal lines as section dividers (set elsewhere via the
    // graphics state; here we just emit a thin underline for the header).
    out.push_str("0.5 w\n0.7 0.7 0.7 RG\n");
    // Header underline at y=730
    out.push_str("50 728 m 562 728 l S\n");
    // Address-block divider at y=540
    out.push_str("50 538 m 562 538 l S\n");
    out.into_bytes()
}

/// Lay out the cover page text. Returns (font, size, x, y, text) tuples.
/// US-Letter PDF coords: 612 wide × 792 tall, origin at bottom-left.
fn lay_out_cover_page(
    name: &str,
    address: &str,
    city_state_zip: &str,
    irs: &IrsAddress,
    irs_with_payment: &IrsAddress,
    paying: bool,
    balance_due_display: &str,
    year: i64,
) -> Vec<(&'static str, f32, f32, f32, String)> {
    // F1 = Helvetica, F2 = Helvetica-Bold (declared in resources below).
    let mut v: Vec<(&'static str, f32, f32, f32, String)> = Vec::new();

    // Header
    v.push(("F2", 18.0, 50.0, 745.0, "MAILING INSTRUCTIONS — IRS FORM 4868".to_string()));
    v.push(("F1", 11.0, 50.0, 712.0, format!("Application for Automatic Extension of Time to File · Tax Year {}", year)));

    // Steps
    let steps_y = 690.0;
    v.push(("F2", 11.0, 50.0, steps_y, "Steps:".to_string()));
    let steps = [
        "1.  Print pages 2-5 of this PDF (the Form 4868 itself is page 2).",
        "2.  Sign and date the form before mailing — without a signature it's not valid.",
        "3.  If you owe, write a check payable to \"United States Treasury\".",
        "4.  Write your SSN, daytime phone, and \"2025 Form 4868\" on the check memo line.",
        "5.  Place form (+ check, if any) in a #10 envelope with the addresses below.",
        "6.  Mail by April 15, 2026. The USPS postmark is the filing date.",
        "7.  Use Certified Mail with Return Receipt for proof of timely filing.",
    ];
    for (i, s) in steps.iter().enumerate() {
        v.push(("F1", 10.0, 65.0, steps_y - 16.0 - (i as f32 * 14.0), s.to_string()));
    }

    // Return address block (top-left of envelope position)
    let ret_y = 510.0;
    v.push(("F2", 10.0, 50.0, ret_y, "FROM (your return address):".to_string()));
    v.push(("F1", 11.0, 65.0, ret_y - 18.0, name.to_string()));
    v.push(("F1", 11.0, 65.0, ret_y - 32.0, address.to_string()));
    v.push(("F1", 11.0, 65.0, ret_y - 46.0, city_state_zip.to_string()));

    // IRS recipient block (positioned right-of-center, where USPS expects it)
    let to_y = 410.0;
    let to_x = 280.0;
    let to_label = if paying {
        format!("TO (you owe {} — enclose check):", balance_due_display)
    } else {
        "TO (no payment — refund or zero balance):".to_string()
    };
    v.push(("F2", 10.0, to_x, to_y, to_label));
    let to_addr = if paying { irs_with_payment } else { irs };
    let lines = to_addr.lines();
    v.push(("F1", 12.0, to_x + 15.0, to_y - 20.0, lines[0].to_string()));
    v.push(("F1", 12.0, to_x + 15.0, to_y - 36.0, lines[1].to_string()));
    v.push(("F1", 12.0, to_x + 15.0, to_y - 52.0, lines[2].to_string()));

    // Alternate address — show the OTHER one too in case they change their
    // mind about including a payment.
    let alt_y = 320.0;
    let alt_label = if paying {
        "Alternate (no payment) — different address:"
    } else {
        "Alternate (if you decide to enclose a check):"
    };
    v.push(("F1", 9.0, 50.0, alt_y, alt_label.to_string()));
    let alt_addr = if paying { irs } else { irs_with_payment };
    let alines = alt_addr.lines();
    v.push(("F1", 9.0, 65.0, alt_y - 12.0, alines[0].to_string()));
    v.push(("F1", 9.0, 65.0, alt_y - 24.0, alines[1].to_string()));
    v.push(("F1", 9.0, 65.0, alt_y - 36.0, alines[2].to_string()));

    // Notes
    let notes_y = 250.0;
    v.push(("F2", 10.0, 50.0, notes_y, "Notes:".to_string()));
    let notes = [
        "·  Don't send Form 1040 with this — Form 4868 + check (if any) is all the IRS expects.",
        "·  Private delivery (UPS/FedEx/DHL) cannot be used for IRS P.O. Box addresses.",
        "·  Filing the extension does NOT extend the time to PAY. Interest + penalties accrue on",
        "   any unpaid balance after April 15, even if you file by the October 15 extended deadline.",
        "·  Paying online at IRS Direct Pay (irs.gov/payments) auto-files the extension — no mail needed.",
        "·  Keep a copy of this form + your USPS receipt with your tax records for at least 3 years.",
    ];
    for (i, s) in notes.iter().enumerate() {
        v.push(("F1", 9.0, 65.0, notes_y - 16.0 - (i as f32 * 12.0), s.to_string()));
    }

    // Footer
    v.push(("F1", 8.0, 50.0, 50.0, format!("Generated by Syntaur Tax Module · {} · Form filled per IRS Form 4868 (2025) AcroForm spec",
        chrono::Utc::now().format("%Y-%m-%d"))));

    v
}

/// Add a generated cover page as the new first page of `doc`.
fn prepend_cover_page(doc: &mut Document, content_bytes: Vec<u8>) -> Result<(), String> {
    // Standard 14 fonts — no font file needed, every PDF reader has them.
    let f_regular = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let f_bold = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold",
    });
    let resources = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => f_regular, "F2" => f_bold },
    });
    let stream = Stream::new(dictionary! {}, content_bytes);
    let content_id = doc.add_object(stream);

    // Find the existing /Pages root.
    let pages_id = doc.catalog().map_err(|e| format!("catalog: {e}"))?
        .get(b"Pages").map_err(|e| format!("/Pages lookup: {e}"))?
        .as_reference().map_err(|e| format!("/Pages ref: {e}"))?;

    // Build the new page object with /Parent pointing at /Pages.
    let new_page = dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Resources" => resources,
        "Contents" => content_id,
    };
    let new_page_id = doc.add_object(new_page);

    // Insert the new page as the first child + bump /Count.
    let pages_dict = doc.get_object_mut(pages_id).map_err(|e| format!("/Pages get: {e}"))?
        .as_dict_mut().map_err(|e| format!("/Pages dict: {e}"))?;
    let mut kids = pages_dict.get(b"Kids").map_err(|e| format!("/Kids: {e}"))?
        .as_array().map_err(|e| format!("/Kids array: {e}"))?
        .clone();
    kids.insert(0, Object::Reference(new_page_id));
    let new_count = kids.len() as i64;
    pages_dict.set(b"Kids".to_vec(), Object::Array(kids));
    pages_dict.set(b"Count".to_vec(), Object::Integer(new_count));
    Ok(())
}

/// Same as `fill_form_4868` but also prepends a cover page with mailing
/// instructions and the IRS service-center address resolved from the
/// taxpayer's state and whether a payment is enclosed.
pub fn fill_form_4868_with_cover(
    data: &Form4868Data,
    paying: bool,
    balance_due_display: &str,
    year: i64,
) -> Result<Vec<u8>, String> {
    let mut doc = Document::load_mem(FORM_4868_BLANK).map_err(|e| format!("load: {e}"))?;
    apply_form_fields(&mut doc, data)?;

    let state_code = data.state.as_deref().unwrap_or("").to_string();
    let irs = irs_mailing_address(&state_code, false);
    let irs_pay = irs_mailing_address(&state_code, true);
    let csz = format!("{}, {} {}",
        data.city.as_deref().unwrap_or(""),
        data.state.as_deref().unwrap_or(""),
        data.zip.as_deref().unwrap_or(""));
    let layout = lay_out_cover_page(
        data.name.as_deref().unwrap_or(""),
        data.address.as_deref().unwrap_or(""),
        csz.trim_matches(|c: char| c == ',' || c.is_whitespace()),
        &irs, &irs_pay, paying, balance_due_display, year,
    );
    let content = build_text_content_stream(&layout);
    prepend_cover_page(&mut doc, content)?;

    let mut buf: Vec<u8> = Vec::with_capacity(FORM_4868_BLANK.len() + 8192);
    doc.save_to(&mut buf).map_err(|e| format!("save: {e}"))?;
    Ok(buf)
}

/// Apply the form-field values + checkboxes to a loaded Form 4868 PDF.
/// Shared between `fill_form_4868` and `fill_form_4868_with_cover`.
fn apply_form_fields(doc: &mut Document, data: &Form4868Data) -> Result<(), String> {
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    let mut applied = 0usize;
    for id in ids {
        let path = match full_field_name(doc, id) {
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
    Ok(())
}

/// Fill Form 4868 with the supplied data and return the resulting PDF bytes.
pub fn fill_form_4868(data: &Form4868Data) -> Result<Vec<u8>, String> {
    let mut doc = Document::load_mem(FORM_4868_BLANK).map_err(|e| format!("load: {e}"))?;
    apply_form_fields(&mut doc, data)?;
    let mut buf: Vec<u8> = Vec::with_capacity(FORM_4868_BLANK.len() + 4096);
    doc.save_to(&mut buf).map_err(|e| format!("save: {e}"))?;
    Ok(buf)
}
