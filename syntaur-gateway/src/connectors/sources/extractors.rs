//! Format-specific text extractors for the RAG knowledge index.
//!
//! Each function takes a file path and returns `Ok(String)` with the extracted
//! plain text, or `Err` on I/O / parse failures. The caller in
//! `uploaded_files.rs` wraps the result in `Option` and logs errors.

use std::io::Read;
use std::path::Path;

/// Extract text from a DOCX file (Office Open XML word processing).
/// Unzips the archive, parses `word/document.xml`, and walks `<w:t>` text nodes.
pub fn extract_docx(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;

    let mut xml = String::new();
    let mut entry = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("no word/document.xml: {e}"))?;
    entry.read_to_string(&mut xml).map_err(|e| format!("read: {e}"))?;

    Ok(extract_xml_text_nodes(&xml, &["w:t", "w:tab", "w:br"]))
}

/// Extract text from a PPTX file (Office Open XML presentation).
/// Iterates over `ppt/slides/slide*.xml` and extracts `<a:t>` text nodes.
pub fn extract_pptx(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;

    let mut slide_names: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                slide_names.push(name);
            }
        }
    }
    slide_names.sort();

    let mut out = String::new();
    for name in &slide_names {
        let mut xml = String::new();
        if let Ok(mut entry) = archive.by_name(name) {
            let _ = entry.read_to_string(&mut xml);
            let text = extract_xml_text_nodes(&xml, &["a:t"]);
            if !text.trim().is_empty() {
                if !out.is_empty() {
                    out.push_str("\n\n---\n\n");
                }
                out.push_str(&text);
            }
        }
    }
    Ok(out)
}

/// Extract text from an ODT file (OpenDocument Text).
/// Parses `content.xml` and walks `<text:p>`, `<text:h>`, `<text:span>` nodes.
pub fn extract_odt(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;

    let mut xml = String::new();
    let mut entry = archive
        .by_name("content.xml")
        .map_err(|e| format!("no content.xml: {e}"))?;
    entry.read_to_string(&mut xml).map_err(|e| format!("read: {e}"))?;

    Ok(extract_odf_text(&xml))
}

/// Extract text from XLSX/XLS/ODS spreadsheets using calamine.
/// Reads all sheets, concatenating cell values row-by-row with tab delimiters.
pub fn extract_spreadsheet(path: &Path) -> Result<String, String> {
    use calamine::{open_workbook_auto, Data, Reader};

    let mut workbook = open_workbook_auto(path).map_err(|e| format!("calamine: {e}"))?;
    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut out = String::new();

    for name in &sheet_names {
        if let Ok(range) = workbook.worksheet_range(name) {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("## {name}\n\n"));
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => format!("{f}"),
                        Data::Int(i) => format!("{i}"),
                        Data::Bool(b) => format!("{b}"),
                        Data::Error(e) => format!("{e:?}"),
                        Data::DateTime(dt) => format!("{dt}"),
                        Data::DateTimeIso(s) => s.clone(),
                        Data::DurationIso(s) => s.clone(),
                    })
                    .collect();
                let line = cells.join("\t");
                if !line.trim().is_empty() {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
        }
    }
    Ok(out)
}

/// Extract text from an EPUB file.
/// Reads the OPF manifest to find content documents in spine order,
/// then extracts text from each XHTML chapter.
pub fn extract_epub(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;

    // Find the rootfile from META-INF/container.xml
    let mut container_xml = String::new();
    {
        let mut entry = archive
            .by_name("META-INF/container.xml")
            .map_err(|e| format!("no container.xml: {e}"))?;
        entry.read_to_string(&mut container_xml).map_err(|e| format!("read: {e}"))?;
    }
    let rootfile = find_opf_path(&container_xml).unwrap_or_else(|| "OEBPS/content.opf".to_string());
    let opf_dir = rootfile.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();

    // Read the OPF to get spine item hrefs
    let mut opf_xml = String::new();
    {
        let mut entry = archive
            .by_name(&rootfile)
            .map_err(|e| format!("no {rootfile}: {e}"))?;
        entry.read_to_string(&mut opf_xml).map_err(|e| format!("read: {e}"))?;
    }
    let hrefs = extract_spine_hrefs(&opf_xml);

    let mut out = String::new();
    for href in &hrefs {
        let full_path = format!("{opf_dir}{href}");
        let mut xhtml = String::new();
        if let Ok(mut entry) = archive.by_name(&full_path) {
            let _ = entry.read_to_string(&mut xhtml);
            let text = strip_html_tags(&xhtml);
            if !text.trim().is_empty() {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&text);
            }
        }
    }
    Ok(out)
}

/// Extract text from an RTF file using basic control-word stripping.
/// Handles common RTF constructs: groups, unicode escapes, special chars.
pub fn extract_rtf(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    Ok(strip_rtf(&raw))
}

/// Extract text from an .eml email file using mailparse.
pub fn extract_eml(path: &Path) -> Result<String, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let mail = mailparse::parse_mail(&raw).map_err(|e| format!("mailparse: {e}"))?;

    let mut out = String::new();

    // Headers
    for key in &["From", "To", "Subject", "Date"] {
        if let Some(val) = mail.headers.iter().find(|h| h.get_key_ref() == *key) {
            let v = val.get_value();
            if !v.is_empty() {
                out.push_str(&format!("{key}: {v}\n"));
            }
        }
    }
    out.push('\n');

    // Body: prefer text/plain, fall back to text/html stripped
    let body = get_mail_text(&mail);
    out.push_str(&body);
    Ok(out)
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Extract text content from XML using quick-xml, looking for specific element names.
/// Used for DOCX (<w:t>) and PPTX (<a:t>).
fn extract_xml_text_nodes(xml: &str, text_tags: &[&str]) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    let mut in_paragraph = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = local_name(e.name().as_ref());
                if text_tags.iter().any(|t| tag_matches(t, &local)) {
                    in_text = true;
                }
                if local == "p" || local == "w:p" || local == "a:p" {
                    if in_paragraph && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    in_paragraph = true;
                }
                if local == "w:tab" || local == "tab" {
                    out.push('\t');
                }
                if local == "w:br" || local == "br" {
                    out.push('\n');
                }
            }
            Ok(Event::Text(e)) if in_text => {
                if let Ok(text) = e.unescape() {
                    out.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                if text_tags.iter().any(|t| tag_matches(t, &local)) {
                    in_text = false;
                }
                if local == "p" || local == "w:p" || local == "a:p" {
                    in_paragraph = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Extract text from ODF content.xml. Walks <text:p> and <text:h> elements,
/// collecting all nested text (including <text:span>).
fn extract_odf_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut depth: u32 = 0; // depth inside text:p or text:h

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "text:p" || local == "text:h" || local == "p" || local == "h" {
                    if depth == 0 && !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    depth += 1;
                } else if depth > 0 {
                    // inside a paragraph, count nested elements
                    if local == "text:tab" || local == "tab" {
                        out.push('\t');
                    }
                    if local == "text:line-break" || local == "line-break" {
                        out.push('\n');
                    }
                }
            }
            Ok(Event::Text(e)) if depth > 0 => {
                if let Ok(text) = e.unescape() {
                    out.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "text:p" || local == "text:h" || local == "p" || local == "h" {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Get the local name portion of an XML tag (after any namespace prefix colon).
fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).to_string()
}

/// Check if a tag name matches, handling namespace prefixes.
fn tag_matches(expected: &str, actual: &str) -> bool {
    actual == expected || actual.ends_with(&format!(":{}", expected.split(':').last().unwrap_or(expected)))
}

/// Find the OPF rootfile path from an EPUB container.xml.
fn find_opf_path(container_xml: &str) -> Option<String> {
    // Look for full-path attribute in <rootfile> element
    let lower = container_xml.to_lowercase();
    let idx = lower.find("full-path=\"")?;
    let start = idx + 11;
    let rest = &container_xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Extract spine item hrefs from an EPUB OPF file.
/// Parses <manifest> for id→href mapping, then reads <spine> itemrefs.
fn extract_spine_hrefs(opf: &str) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut manifest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut spine_ids: Vec<String> = Vec::new();

    let mut reader = Reader::from_str(opf);
    let mut in_spine = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "item" || local.ends_with(":item") {
                    let mut id = String::new();
                    let mut href = String::new();
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "id" { id = val.clone(); }
                        if key == "href" { href = val; }
                    }
                    if !id.is_empty() && !href.is_empty() {
                        manifest.insert(id, href);
                    }
                }
                if local == "spine" || local.ends_with(":spine") {
                    in_spine = true;
                }
                if in_spine && (local == "itemref" || local.ends_with(":itemref")) {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        if key == "idref" {
                            spine_ids.push(String::from_utf8_lossy(&attr.value).to_string());
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                if local == "spine" || local.ends_with(":spine") {
                    in_spine = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    spine_ids
        .iter()
        .filter_map(|id| manifest.get(id).cloned())
        .filter(|href| href.ends_with(".xhtml") || href.ends_with(".html") || href.ends_with(".htm") || href.ends_with(".xml"))
        .collect()
}

/// Strip HTML/XHTML tags, keeping only text content. Basic but sufficient for
/// EPUB chapter extraction and HTML email fallback.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Check for script/style start
            if i + 7 < lower_bytes.len() && &lower_bytes[i..i + 7] == b"<script" {
                in_script = true;
            }
            if i + 6 < lower_bytes.len() && &lower_bytes[i..i + 6] == b"<style" {
                in_style = true;
            }
            // Check for script/style end
            if i + 9 < lower_bytes.len() && &lower_bytes[i..i + 9] == b"</script>" {
                in_script = false;
                i += 9;
                continue;
            }
            if i + 8 < lower_bytes.len() && &lower_bytes[i..i + 8] == b"</style>" {
                in_style = false;
                i += 8;
                continue;
            }
            in_tag = true;
            // Block-level elements get a newline
            if i + 3 < lower_bytes.len() {
                let after = &lower_bytes[i + 1..];
                let block_tags: &[&[u8]] = &[b"p>", b"p ", b"br", b"div", b"h1>", b"h2>", b"h3>", b"h4>", b"h5>", b"h6>", b"li", b"tr"];
                for tag in block_tags {
                    if after.starts_with(tag) && !out.ends_with('\n') {
                        out.push('\n');
                        break;
                    }
                }
            }
        } else if bytes[i] == b'>' {
            in_tag = false;
        } else if !in_tag && !in_script && !in_style {
            out.push(bytes[i] as char);
        }
        i += 1;
    }

    // Decode common HTML entities
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Strip RTF control words and groups, extracting plain text.
fn strip_rtf(rtf: &str) -> String {
    let mut out = String::new();
    let bytes = rtf.as_bytes();
    let mut i = 0;
    let mut group_depth: i32 = 0;
    let mut skip_group_depth: Option<i32> = None;

    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                group_depth += 1;
                i += 1;
            }
            b'}' => {
                if skip_group_depth == Some(group_depth) {
                    skip_group_depth = None;
                }
                group_depth -= 1;
                i += 1;
            }
            b'\\' if skip_group_depth.is_none() => {
                i += 1;
                if i >= bytes.len() { break; }
                match bytes[i] {
                    b'\'' => {
                        // Hex escape \'XX
                        if i + 2 < bytes.len() {
                            let hex = &rtf[i + 1..i + 3];
                            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                                out.push(byte as char);
                            }
                            i += 3;
                        } else {
                            i += 1;
                        }
                    }
                    b'u' => {
                        // Unicode escape \uN
                        i += 1;
                        let start = i;
                        while i < bytes.len() && (bytes[i].is_ascii_digit() || (i == start && bytes[i] == b'-')) {
                            i += 1;
                        }
                        if let Ok(code) = rtf[start..i].parse::<i32>() {
                            let code = if code < 0 { (code + 65536) as u32 } else { code as u32 };
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                        }
                        // Skip replacement char
                        if i < bytes.len() && bytes[i] == b' ' { i += 1; }
                    }
                    b'\n' | b'\r' => {
                        out.push('\n');
                        i += 1;
                    }
                    _ => {
                        // Control word
                        let start = i;
                        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        let word = &rtf[start..i];
                        // Skip numeric parameter
                        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'-') {
                            i += 1;
                        }
                        // Trailing space delimiter
                        if i < bytes.len() && bytes[i] == b' ' { i += 1; }
                        match word {
                            "par" | "line" => out.push('\n'),
                            "tab" => out.push('\t'),
                            // Skip content in these destination groups
                            "fonttbl" | "colortbl" | "stylesheet" | "info"
                            | "header" | "footer" | "pict" | "object" => {
                                skip_group_depth = Some(group_depth);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ if skip_group_depth.is_some() => { i += 1; }
            b'\r' | b'\n' => { i += 1; }
            _ => {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
    }
    out
}

/// Recursively extract text/plain from a parsed email, falling back to
/// stripped text/html if no plain part exists.
fn get_mail_text(mail: &mailparse::ParsedMail) -> String {
    // Single part
    if mail.subparts.is_empty() {
        let ct = mail.ctype.mimetype.to_lowercase();
        if ct == "text/plain" {
            return mail.get_body().unwrap_or_default();
        }
        if ct == "text/html" {
            return strip_html_tags(&mail.get_body().unwrap_or_default());
        }
        return String::new();
    }

    // Multipart: look for text/plain first, then text/html
    let mut plain = String::new();
    let mut html = String::new();
    for part in &mail.subparts {
        let ct = part.ctype.mimetype.to_lowercase();
        if ct == "text/plain" && plain.is_empty() {
            plain = part.get_body().unwrap_or_default();
        } else if ct == "text/html" && html.is_empty() {
            html = part.get_body().unwrap_or_default();
        } else if ct.starts_with("multipart/") {
            let nested = get_mail_text(part);
            if !nested.is_empty() && plain.is_empty() {
                plain = nested;
            }
        }
    }
    if !plain.is_empty() { plain } else { strip_html_tags(&html) }
}
