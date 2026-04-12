use std::path::{Path, PathBuf};

/// Validate and resolve a path within the workspace
fn safe_path(workspace: &Path, relative: &str) -> Result<PathBuf, String> {
    let expanded = relative.replace("~", &std::env::var("HOME").unwrap_or_default());

    let path = if Path::new(&expanded).is_absolute() {
        PathBuf::from(&expanded)
    } else {
        workspace.join(&expanded)
    };

    // Canonicalize to resolve .. and symlinks
    let canonical = path.canonicalize()
        .or_else(|_| {
            // File might not exist yet (for write). Check parent.
            if let Some(parent) = path.parent() {
                parent.canonicalize().map(|p| p.join(path.file_name().unwrap_or_default()))
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "invalid path"))
            }
        })
        .map_err(|e| format!("Invalid path '{}': {}", relative, e))?;

    // Check that the path is within allowed directories
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let allowed_prefixes = [
        workspace.to_path_buf(),
        PathBuf::from(format!("{}/.syntaur", home)),
        PathBuf::from(format!("{}/.syntaur", home)),
        PathBuf::from("/tmp/syntaur"),
    ];

    let is_allowed = allowed_prefixes.iter().any(|prefix| {
        if let Ok(prefix_canon) = prefix.canonicalize() {
            canonical.starts_with(&prefix_canon)
        } else {
            canonical.starts_with(prefix)
        }
    });

    if !is_allowed {
        return Err(format!("Path '{}' is outside allowed directories", relative));
    }

    Ok(canonical)
}

pub fn read_file(workspace: &Path, path: &str) -> Result<String, String> {
    let safe = safe_path(workspace, path)?;
    std::fs::read_to_string(&safe)
        .map_err(|e| format!("Cannot read '{}': {}", path, e))
}

pub fn write_file(workspace: &Path, path: &str, content: &str) -> Result<String, String> {
    let expanded = path.replace("~", &std::env::var("HOME").unwrap_or_default());
    let full_path = if Path::new(&expanded).is_absolute() {
        PathBuf::from(&expanded)
    } else {
        workspace.join(&expanded)
    };

    // Validate parent directory is in allowed paths
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create directory: {}", e))?;
    }

    // Check path is allowed
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/sean".to_string());
    let is_allowed = full_path.starts_with(workspace)
        || full_path.starts_with(format!("{}/.syntaur", home))
        || full_path.starts_with("/tmp/syntaur");

    if !is_allowed {
        return Err(format!("Cannot write outside allowed directories: {}", path));
    }

    std::fs::write(&full_path, content)
        .map_err(|e| format!("Cannot write '{}': {}", path, e))?;

    Ok(format!("Written {} bytes to {}", content.len(), path))
}

pub fn edit_file(workspace: &Path, path: &str, old: &str, new: &str) -> Result<String, String> {
    let safe = safe_path(workspace, path)?;
    let content = std::fs::read_to_string(&safe)
        .map_err(|e| format!("Cannot read '{}': {}", path, e))?;

    if !content.contains(old) {
        return Err(format!("String not found in '{}'", path));
    }

    let updated = content.replacen(old, new, 1);
    std::fs::write(&safe, &updated)
        .map_err(|e| format!("Cannot write '{}': {}", path, e))?;

    Ok(format!("Edited {}", path))
}

pub fn list_files(workspace: &Path, path: &str) -> Result<String, String> {
    let safe = safe_path(workspace, path)?;

    if !safe.is_dir() {
        return Err(format!("'{}' is not a directory", path));
    }

    let mut entries = Vec::new();
    let dir = std::fs::read_dir(&safe)
        .map_err(|e| format!("Cannot list '{}': {}", path, e))?;

    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let ft = entry.file_type().ok();
        let prefix = if ft.map_or(false, |t| t.is_dir()) { "d " } else { "f " };
        entries.push(format!("{}{}", prefix, name));
    }

    entries.sort();
    Ok(entries.join("\n"))
}
