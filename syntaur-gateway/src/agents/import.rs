//! Agent import — parse an uploaded prompt file into `ImportedAgent`.
//!
//! Supports:
//! - `.md` / `.txt` — treat the body as the system prompt, extract the name
//!   from the first H1 (Markdown) or the filename.
//! - `.json` — detect Claude Project export, ChatGPT custom-GPT export, or
//!   a generic `{name, description, system_prompt}` shape.
//!
//! Returns enough information to create a `user_agents` row.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedAgent {
    pub name: String,
    pub description: Option<String>,
    pub system_prompt: String,
    pub source_format: &'static str,
}

/// Parse a file's bytes into an imported agent. `original_filename` is used as
/// a name fallback for plain-text / Markdown uploads that don't include a
/// title header.
pub fn parse_file(filename: &str, bytes: &[u8]) -> Result<ImportedAgent, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "File is not valid UTF-8 text.".to_string())?;
    let lower = filename.to_lowercase();

    if lower.ends_with(".json") {
        parse_json(text).map(|mut a| {
            if a.name.trim().is_empty() {
                a.name = stem(&lower).to_string();
            }
            a
        })
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        Ok(parse_markdown(text, &lower))
    } else if lower.ends_with(".txt") || !lower.contains('.') {
        Ok(parse_text(text, &lower))
    } else {
        // Unknown extension — fall back to plain text. Better than erroring
        // when the file is clearly a prompt with a weird suffix.
        Ok(parse_text(text, &lower))
    }
}

fn stem(filename: &str) -> &str {
    let last_slash = filename.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let after = &filename[last_slash..];
    match after.rfind('.') {
        Some(i) => &after[..i],
        None => after,
    }
}

fn pretty_name_from_stem(stem: &str) -> String {
    // Turn "my-custom-agent" / "my_agent" into "My Custom Agent".
    stem.split(|c: char| c == '-' || c == '_' || c == ' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_markdown(text: &str, filename: &str) -> ImportedAgent {
    let mut lines = text.lines();
    // Optional YAML frontmatter: ----- blocks at the top.
    let mut fm_name: Option<String> = None;
    let mut fm_description: Option<String> = None;
    let mut first_line = lines.next().unwrap_or("");
    if first_line.trim() == "---" {
        // Consume frontmatter lines until the closing '---'.
        for line in lines.by_ref() {
            if line.trim() == "---" { break; }
            if let Some(v) = line.strip_prefix("name:") {
                fm_name = Some(v.trim().trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("description:") {
                fm_description = Some(v.trim().trim_matches('"').to_string());
            }
        }
        first_line = lines.next().unwrap_or("");
    }
    // Collect the rest into a body so we can scan for H1.
    let rest: Vec<&str> = std::iter::once(first_line).chain(lines).collect();

    // Name: prefer frontmatter, then first H1, then filename stem.
    let mut name = fm_name.unwrap_or_default();
    let mut body_start = 0;
    for (i, l) in rest.iter().enumerate() {
        if let Some(title) = l.strip_prefix("# ") {
            if name.is_empty() {
                name = title.trim().to_string();
            }
            body_start = i + 1;
            break;
        }
        if !l.trim().is_empty() { break; }
    }
    if name.is_empty() {
        name = pretty_name_from_stem(stem(filename));
    }

    let body: String = rest[body_start..].join("\n").trim().to_string();
    let system_prompt = if body.is_empty() { text.trim().to_string() } else { body };

    // Description: frontmatter wins; else first non-heading paragraph (max 200 chars).
    let description = fm_description.or_else(|| {
        system_prompt
            .lines()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#') && !t.starts_with('-') && !t.starts_with('*')
            })
            .map(|l| {
                let t = l.trim().to_string();
                if t.len() > 200 { format!("{}…", &t[..200]) } else { t }
            })
    });

    ImportedAgent {
        name,
        description,
        system_prompt,
        source_format: "markdown",
    }
}

fn parse_text(text: &str, filename: &str) -> ImportedAgent {
    ImportedAgent {
        name: pretty_name_from_stem(stem(filename)),
        description: None,
        system_prompt: text.trim().to_string(),
        source_format: "text",
    }
}

fn parse_json(text: &str) -> Result<ImportedAgent, String> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("Not valid JSON: {e}"))?;

    // Strategy: probe known fields in priority order. This covers
    // ChatGPT custom-GPT exports, Claude Projects, and the generic shape
    // described on the Settings page.
    let name = first_string(&v, &[
        &["name"],
        &["display_name"],
        &["agent_name"],
        &["gpt", "name"],           // ChatGPT export
        &["project", "name"],       // Claude Projects
        &["title"],
    ]).unwrap_or_default();

    let description = first_string(&v, &[
        &["description"],
        &["summary"],
        &["gpt", "description"],
        &["project", "description"],
    ]);

    let system_prompt = first_string(&v, &[
        &["system_prompt"],
        &["instructions"],
        &["prompt"],
        &["system"],
        &["gpt", "instructions"],   // ChatGPT export
        &["project", "instructions"],
        &["project", "system_prompt"],
    ]).ok_or_else(|| {
        "JSON file is missing a recognizable prompt field (expected one of: \
        `system_prompt`, `instructions`, `prompt`, `system`, `gpt.instructions`, \
        `project.instructions`).".to_string()
    })?;

    let source_format = if v.pointer("/gpt/instructions").is_some() {
        "chatgpt_gpt"
    } else if v.pointer("/project/instructions").is_some() || v.pointer("/project/system_prompt").is_some() {
        "claude_project"
    } else {
        "json"
    };

    Ok(ImportedAgent {
        name,
        description,
        system_prompt: system_prompt.trim().to_string(),
        source_format,
    })
}

/// Return the first string found at any of the candidate JSON paths.
fn first_string(v: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut node = v;
        let mut ok = true;
        for seg in *path {
            match node.get(*seg) {
                Some(inner) => node = inner,
                None => { ok = false; break; }
            }
        }
        if ok {
            if let Some(s) = node.as_str() {
                if !s.trim().is_empty() {
                    return Some(s.trim().to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_with_h1() {
        let a = parse_file("prompt.md", b"# Alice\n\nYou are Alice, a helpful assistant.").unwrap();
        assert_eq!(a.name, "Alice");
        assert!(a.system_prompt.contains("You are Alice"));
        assert_eq!(a.source_format, "markdown");
    }

    #[test]
    fn markdown_with_frontmatter() {
        let input = "---\nname: \"Bob\"\ndescription: \"a tester\"\n---\nSystem prompt body.\n";
        let a = parse_file("anything.md", input.as_bytes()).unwrap();
        assert_eq!(a.name, "Bob");
        assert_eq!(a.description.as_deref(), Some("a tester"));
    }

    #[test]
    fn text_uses_filename() {
        let a = parse_file("my-custom-agent.txt", b"You are helpful.").unwrap();
        assert_eq!(a.name, "My Custom Agent");
        assert_eq!(a.system_prompt, "You are helpful.");
    }

    #[test]
    fn json_chatgpt_shape() {
        let j = r#"{"gpt":{"name":"Gene","description":"the explainer","instructions":"You are Gene."}}"#;
        let a = parse_file("export.json", j.as_bytes()).unwrap();
        assert_eq!(a.name, "Gene");
        assert_eq!(a.description.as_deref(), Some("the explainer"));
        assert_eq!(a.source_format, "chatgpt_gpt");
    }

    #[test]
    fn json_claude_project() {
        let j = r#"{"project":{"name":"Cora","instructions":"You are Cora."}}"#;
        let a = parse_file("export.json", j.as_bytes()).unwrap();
        assert_eq!(a.name, "Cora");
        assert_eq!(a.source_format, "claude_project");
    }

    #[test]
    fn json_generic() {
        let j = r#"{"name":"Dave","system_prompt":"You are Dave."}"#;
        let a = parse_file("a.json", j.as_bytes()).unwrap();
        assert_eq!(a.name, "Dave");
        assert_eq!(a.source_format, "json");
    }

    #[test]
    fn json_missing_prompt_errors() {
        let j = r#"{"name":"X"}"#;
        assert!(parse_file("a.json", j.as_bytes()).is_err());
    }
}
