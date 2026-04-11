use std::path::Path;

/// Read memory files from agent workspace
pub fn read_memory(workspace: &Path, query: &str) -> Result<String, String> {
    let memory_dir = workspace.join("memory");

    if query.is_empty() {
        // List available memory files
        if !memory_dir.exists() {
            return Ok("No memory directory".to_string());
        }
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    files.push(name);
                }
            }
        }
        files.sort();
        return Ok(files.join("\n"));
    }

    // Try exact file match first
    let file_path = if query.ends_with(".md") {
        memory_dir.join(query)
    } else {
        memory_dir.join(format!("{}.md", query))
    };

    if file_path.exists() {
        return std::fs::read_to_string(&file_path)
            .map_err(|e| format!("Cannot read memory file: {}", e));
    }

    // Also check workspace root for MEMORY.md, CURRENT_STATE.md, etc.
    let root_file = workspace.join(query);
    if root_file.exists() {
        return std::fs::read_to_string(&root_file)
            .map_err(|e| format!("Cannot read: {}", e));
    }

    // Search memory files for content
    let mut results = Vec::new();
    if memory_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "md") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let query_lower = query.to_lowercase();
                        if content.to_lowercase().contains(&query_lower) {
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            results.push(format!("--- {} ---\n{}", name, content));
                        }
                    }
                }
            }
        }
    }

    if results.is_empty() {
        Err(format!("No memory matching '{}'", query))
    } else {
        Ok(results.join("\n\n"))
    }
}

/// Write to agent memory
pub fn write_memory(workspace: &Path, key: &str, content: &str) -> Result<String, String> {
    let memory_dir = workspace.join("memory");
    std::fs::create_dir_all(&memory_dir)
        .map_err(|e| format!("Cannot create memory dir: {}", e))?;

    let filename = if key.ends_with(".md") {
        key.to_string()
    } else {
        format!("{}.md", key)
    };

    let path = memory_dir.join(&filename);
    std::fs::write(&path, content)
        .map_err(|e| format!("Cannot write memory: {}", e))?;

    Ok(format!("Memory written: {}", filename))
}
