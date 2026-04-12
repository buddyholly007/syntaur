//! Tool definitions and dispatch.
//!
//! Tool names, descriptions, and JSON schemas are intentionally chosen to
//! match `@modelcontextprotocol/server-filesystem` so this binary is a
//! drop-in replacement for it.

use std::path::PathBuf;

use async_trait::async_trait;
use base64::Engine as _;
use globset::{Glob, GlobSet, GlobSetBuilder};
use mcp_protocol::server::{ServerHandler, ToolCallResult, ToolDef};
use mcp_protocol::messages::ServerInfo;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ops::{
    self, apply_file_edits, format_size, format_time, get_file_stats, head_file, tail_file,
    write_file_content, EditOperation,
};
use crate::paths::validate_path;

pub struct FilesystemHandler {
    allowed: Vec<PathBuf>,
}

impl FilesystemHandler {
    pub fn new(allowed: Vec<PathBuf>) -> Self {
        Self { allowed }
    }
}

// ---- argument schemas ----

#[derive(Deserialize)]
struct ReadTextArgs {
    path: String,
    #[serde(default)]
    head: Option<usize>,
    #[serde(default)]
    tail: Option<usize>,
}

#[derive(Deserialize)]
struct PathArg {
    path: String,
}

#[derive(Deserialize)]
struct ReadMultipleArgs {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct EditOpJson {
    #[serde(rename = "oldText")]
    old_text: String,
    #[serde(rename = "newText")]
    new_text: String,
}

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    edits: Vec<EditOpJson>,
    #[serde(default)]
    #[serde(rename = "dryRun")]
    dry_run: bool,
}

#[derive(Deserialize)]
struct ListWithSizesArgs {
    path: String,
    #[serde(default = "default_sort")]
    #[serde(rename = "sortBy")]
    sort_by: String,
}

fn default_sort() -> String {
    "name".to_string()
}

#[derive(Deserialize)]
struct DirectoryTreeArgs {
    path: String,
    #[serde(default)]
    #[serde(rename = "excludePatterns")]
    exclude_patterns: Vec<String>,
}

#[derive(Deserialize)]
struct MoveFileArgs {
    source: String,
    destination: String,
}

#[derive(Deserialize)]
struct SearchFilesArgs {
    path: String,
    pattern: String,
    #[serde(default)]
    #[serde(rename = "excludePatterns")]
    exclude_patterns: Vec<String>,
}

// ---- handler ----

#[async_trait]
impl ServerHandler for FilesystemHandler {
    fn server_info(&self) -> ServerInfo {
        ServerInfo {
            name: "secure-filesystem-server".to_string(),
            version: "0.2.0".to_string(),
        }
    }

    fn tools(&self) -> Vec<ToolDef> {
        tool_defs()
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> ToolCallResult {
        let allowed = self.allowed.clone();
        let name = name.to_string();
        let args = arguments.clone();

        // All filesystem ops are blocking; run them on the dedicated pool.
        let res = tokio::task::spawn_blocking(move || dispatch(&name, args, &allowed)).await;

        match res {
            Ok(Ok(text)) => ToolCallResult::text(text),
            Ok(Err(e)) => ToolCallResult::error(e),
            Err(join_err) => ToolCallResult::error(format!("internal panic: {}", join_err)),
        }
    }
}

fn dispatch(name: &str, args: Value, allowed: &[PathBuf]) -> Result<String, String> {
    match name {
        // Deprecated alias for read_text_file but the Node ref still ships it.
        "read_file" | "read_text_file" => {
            let a: ReadTextArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            if a.head.is_some() && a.tail.is_some() {
                return Err("Cannot specify both head and tail parameters simultaneously".into());
            }
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let content = if let Some(n) = a.tail {
                tail_file(&v.real, n).map_err(|e| e.to_string())?
            } else if let Some(n) = a.head {
                head_file(&v.real, n).map_err(|e| e.to_string())?
            } else {
                ops::read_text(&v.real).map_err(|e| e.to_string())?
            };
            Ok(content)
        }

        "read_media_file" => {
            let a: PathArg = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let bytes = ops::read_bytes(&v.real).map_err(|e| e.to_string())?;
            let mime = mime_guess::from_path(&v.real)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            let kind = if mime.starts_with("image/") {
                "image"
            } else if mime.starts_with("audio/") {
                "audio"
            } else {
                "blob"
            };
            // The Syntaur MCP client only forwards text-content blocks back
            // to the LLM, so we serialize the media as a JSON envelope inside
            // a text block. Same data, just one layer of unwrapping for the
            // model. Future work: extend the protocol layer to carry typed
            // content end-to-end.
            let value = json!({
                "type": kind,
                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "mimeType": mime,
            });
            Ok(value.to_string())
        }

        "read_multiple_files" => {
            let a: ReadMultipleArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let mut parts = Vec::with_capacity(a.paths.len());
            for p in &a.paths {
                match validate_path(p, allowed) {
                    Ok(v) => match ops::read_text(&v.real) {
                        Ok(c) => parts.push(format!("{}:\n{}\n", p, c)),
                        Err(e) => parts.push(format!("{}: Error - {}", p, e)),
                    },
                    Err(e) => parts.push(format!("{}: Error - {}", p, e)),
                }
            }
            Ok(parts.join("\n---\n"))
        }

        "write_file" => {
            let a: WriteFileArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            write_file_content(&v.real, &a.content).map_err(|e| e.to_string())?;
            Ok(format!("Successfully wrote to {}", a.path))
        }

        "edit_file" => {
            let a: EditFileArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let edits: Vec<EditOperation> = a
                .edits
                .into_iter()
                .map(|e| EditOperation {
                    old_text: e.old_text,
                    new_text: e.new_text,
                })
                .collect();
            apply_file_edits(&v.real, &edits, a.dry_run).map_err(|e| e.to_string())
        }

        "create_directory" => {
            let a: PathArg = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&v.real).map_err(|e| e.to_string())?;
            Ok(format!("Successfully created directory {}", a.path))
        }

        "list_directory" => {
            let a: PathArg = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let entries = std::fs::read_dir(&v.real).map_err(|e| e.to_string())?;
            let mut lines = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| e.to_string())?;
                let ty = entry.file_type().map_err(|e| e.to_string())?;
                let prefix = if ty.is_dir() { "[DIR]" } else { "[FILE]" };
                lines.push(format!("{} {}", prefix, entry.file_name().to_string_lossy()));
            }
            Ok(lines.join("\n"))
        }

        "list_directory_with_sizes" => {
            let a: ListWithSizesArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let mut detailed = Vec::new();
            for entry in std::fs::read_dir(&v.real).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let ty = entry.file_type().map_err(|e| e.to_string())?;
                let (size, mtime) = entry
                    .metadata()
                    .map(|m| {
                        (
                            m.len(),
                            m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                        )
                    })
                    .unwrap_or((0, std::time::SystemTime::UNIX_EPOCH));
                detailed.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    ty.is_dir(),
                    size,
                    mtime,
                ));
            }

            if a.sort_by == "size" {
                detailed.sort_by(|a, b| b.2.cmp(&a.2));
            } else {
                detailed.sort_by(|a, b| a.0.cmp(&b.0));
            }

            let mut total_files = 0u64;
            let mut total_dirs = 0u64;
            let mut total_size = 0u64;
            let mut lines = Vec::new();
            for (name, is_dir, size, _mtime) in &detailed {
                let prefix = if *is_dir { "[DIR]" } else { "[FILE]" };
                let size_str = if *is_dir {
                    String::new()
                } else {
                    format!("{:>10}", format_size(*size))
                };
                lines.push(format!("{} {:30} {}", prefix, name, size_str));
                if *is_dir {
                    total_dirs += 1;
                } else {
                    total_files += 1;
                    total_size += *size;
                }
            }
            lines.push(String::new());
            lines.push(format!(
                "Total: {} files, {} directories",
                total_files, total_dirs
            ));
            lines.push(format!("Combined size: {}", format_size(total_size)));
            Ok(lines.join("\n"))
        }

        "directory_tree" => {
            let a: DirectoryTreeArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let root = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let exclude = build_globset(&a.exclude_patterns).map_err(|e| e.to_string())?;
            let tree = build_tree(&root.real, &root.real, &exclude).map_err(|e| e.to_string())?;
            Ok(serde_json::to_string_pretty(&tree).unwrap_or_default())
        }

        "move_file" => {
            let a: MoveFileArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let src = validate_path(&a.source, allowed).map_err(|e| e.to_string())?;
            let dst = validate_path(&a.destination, allowed).map_err(|e| e.to_string())?;
            std::fs::rename(&src.real, &dst.real).map_err(|e| e.to_string())?;
            Ok(format!(
                "Successfully moved {} to {}",
                a.source, a.destination
            ))
        }

        "search_files" => {
            let a: SearchFilesArgs = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let root = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let pattern_set = build_globset(&[a.pattern]).map_err(|e| e.to_string())?;
            let exclude = build_globset(&a.exclude_patterns).map_err(|e| e.to_string())?;
            let mut matches = Vec::new();
            walk_search(&root.real, &root.real, allowed, &pattern_set, &exclude, &mut matches)
                .map_err(|e| e.to_string())?;
            if matches.is_empty() {
                Ok("No matches found".to_string())
            } else {
                Ok(matches
                    .into_iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }

        "get_file_info" => {
            let a: PathArg = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let v = validate_path(&a.path, allowed).map_err(|e| e.to_string())?;
            let info = get_file_stats(&v.real).map_err(|e| e.to_string())?;
            let mut lines = vec![
                format!("size: {}", info.size),
                format!("created: {}", format_time(info.created)),
                format!("modified: {}", format_time(info.modified)),
                format!("accessed: {}", format_time(info.accessed)),
                format!("isDirectory: {}", info.is_directory),
                format!("isFile: {}", info.is_file),
                format!("permissions: {}", info.permissions),
            ];
            // Final newline matches the Node ref's join('\n') with no trailing.
            lines.shrink_to_fit();
            Ok(lines.join("\n"))
        }

        "list_allowed_directories" => Ok(format!(
            "Allowed directories:\n{}",
            allowed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )),

        _ => Err(format!("unknown tool: {}", name)),
    }
}

#[derive(serde::Serialize)]
struct TreeNode {
    name: String,
    #[serde(rename = "type")]
    node_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<TreeNode>>,
}

fn build_tree(
    current: &std::path::Path,
    root: &std::path::Path,
    exclude: &GlobSet,
) -> std::io::Result<Vec<TreeNode>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let full = current.join(&name);
        let rel = full.strip_prefix(root).unwrap_or(&full).to_path_buf();
        if exclude.is_match(&rel) {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_dir() {
            let children = build_tree(&full, root, exclude)?;
            out.push(TreeNode {
                name,
                node_type: "directory",
                children: Some(children),
            });
        } else {
            out.push(TreeNode {
                name,
                node_type: "file",
                children: None,
            });
        }
    }
    Ok(out)
}

fn walk_search(
    current: &std::path::Path,
    root: &std::path::Path,
    allowed: &[PathBuf],
    pattern: &GlobSet,
    exclude: &GlobSet,
    out: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    let entries = std::fs::read_dir(current)?;
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let full = entry.path();
        // Re-validate so symlinks that point outside the sandbox are skipped.
        let real = match std::fs::canonicalize(&full) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !crate::paths::is_within_allowed(&real, allowed) {
            continue;
        }
        let rel = full.strip_prefix(root).unwrap_or(&full).to_path_buf();
        if exclude.is_match(&rel) {
            continue;
        }
        if pattern.is_match(&rel) {
            out.push(full.clone());
        }
        if let Ok(ty) = entry.file_type() {
            if ty.is_dir() {
                let _ = walk_search(&full, root, allowed, pattern, exclude, out);
            }
        }
    }
    Ok(())
}

fn build_globset(patterns: &[String]) -> Result<GlobSet, String> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| format!("bad glob '{}': {}", p, e))?;
        b.add(g);
    }
    b.build().map_err(|e| e.to_string())
}

// ---- tool definitions ----

fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read_file",
            description: "Read the complete contents of a file as text. DEPRECATED: Use read_text_file instead.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "head": {"type": "number", "description": "If provided, returns only the first N lines of the file"},
                    "tail": {"type": "number", "description": "If provided, returns only the last N lines of the file"}
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "read_text_file",
            description: "Read the complete contents of a file from the file system as text. Handles various text encodings and provides detailed error messages if the file cannot be read. Use this tool when you need to examine the contents of a single file. Use the 'head' parameter to read only the first N lines of a file, or the 'tail' parameter to read only the last N lines of a file. Operates on the file as text regardless of extension. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "head": {"type": "number", "description": "If provided, returns only the first N lines of the file"},
                    "tail": {"type": "number", "description": "If provided, returns only the last N lines of the file"}
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "read_media_file",
            description: "Read an image or audio file. Returns the base64 encoded data and MIME type. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "read_multiple_files",
            description: "Read the contents of multiple files simultaneously. This is more efficient than reading files one by one when you need to analyze or compare multiple files. Each file's content is returned with its path as a reference. Failed reads for individual files won't stop the entire operation. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "description": "Array of file paths to read. Each path must be a string pointing to a valid file within allowed directories."
                    }
                },
                "required": ["paths"]
            }),
        },
        ToolDef {
            name: "write_file",
            description: "Create a new file or completely overwrite an existing file with new content. Use with caution as it will overwrite existing files without warning. Handles text content with proper encoding. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolDef {
            name: "edit_file",
            description: "Make line-based edits to a text file. Each edit replaces exact line sequences with new content. Returns a git-style diff showing the changes made. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "oldText": {"type": "string", "description": "Text to search for - must match exactly"},
                                "newText": {"type": "string", "description": "Text to replace with"}
                            },
                            "required": ["oldText", "newText"]
                        }
                    },
                    "dryRun": {"type": "boolean", "default": false, "description": "Preview changes using git-style diff format"}
                },
                "required": ["path", "edits"]
            }),
        },
        ToolDef {
            name: "create_directory",
            description: "Create a new directory or ensure a directory exists. Can create multiple nested directories in one operation. If the directory already exists, this operation will succeed silently. Perfect for setting up directory structures for projects or ensuring required paths exist. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "list_directory",
            description: "Get a detailed listing of all files and directories in a specified path. Results clearly distinguish between files and directories with [FILE] and [DIR] prefixes. This tool is essential for understanding directory structure and finding specific files within a directory. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "list_directory_with_sizes",
            description: "Get a detailed listing of all files and directories in a specified path, including sizes. Results clearly distinguish between files and directories with [FILE] and [DIR] prefixes. This tool is useful for understanding directory structure and finding specific files within a directory. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "sortBy": {"type": "string", "enum": ["name", "size"], "default": "name", "description": "Sort entries by name or size"}
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "directory_tree",
            description: "Get a recursive tree view of files and directories as a JSON structure. Each entry includes 'name', 'type' (file/directory), and 'children' for directories. Files have no children array, while directories always have a children array (which may be empty). The output is formatted with 2-space indentation for readability. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "excludePatterns": {"type": "array", "items": {"type": "string"}, "default": []}
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "move_file",
            description: "Move or rename files and directories. Can move files between directories and rename them in a single operation. If the destination exists, the operation will fail. Works across different directories and can be used for simple renaming within the same directory. Both source and destination must be within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string"},
                    "destination": {"type": "string"}
                },
                "required": ["source", "destination"]
            }),
        },
        ToolDef {
            name: "search_files",
            description: "Recursively search for files and directories matching a pattern. The patterns should be glob-style patterns that match paths relative to the working directory. Use pattern like '*.ext' to match files in current directory, and '**/*.ext' to match files in all subdirectories. Returns full paths to all matching items. Great for finding files when you don't know their exact location. Only searches within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "pattern": {"type": "string"},
                    "excludePatterns": {"type": "array", "items": {"type": "string"}, "default": []}
                },
                "required": ["path", "pattern"]
            }),
        },
        ToolDef {
            name: "get_file_info",
            description: "Retrieve detailed metadata about a file or directory. Returns comprehensive information including size, creation time, last modified time, permissions, and type. This tool is perfect for understanding file characteristics without reading the actual content. Only works within allowed directories.",
            input_schema: json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "list_allowed_directories",
            description: "Returns the list of directories that this server is allowed to access. Subdirectories within these allowed directories are also accessible. Use this to understand which directories and their nested paths are available before trying to access files.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

