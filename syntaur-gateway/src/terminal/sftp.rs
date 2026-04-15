//! SFTP operations over SSH channels.
//! Phase 3: uses russh-sftp over existing SSH connections.

use std::sync::Arc;

use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

#[derive(Deserialize)]
pub struct SftpQuery {
    pub path: Option<String>,
}

/// GET /api/terminal/sftp/{host_id}/ls
pub async fn list_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Query(q): Query<SftpQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = super::hosts::get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let path = q.path.as_deref().unwrap_or("/home/sean");

    if host.is_local {
        // Local filesystem listing
        let entries = std::fs::read_dir(path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut files = Vec::new();
        for entry in entries.flatten() {
            let meta = entry.metadata().ok();
            files.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }

        return Ok(Json(json!({ "path": path, "entries": files })));
    }

    // Remote: SSH + SFTP
    let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
    let key_path_expanded = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());

    // Use ssh command for now (russh-sftp integration in Phase 3+)
    let output = tokio::process::Command::new("ssh")
        .args([
            "-i", &key_path_expanded,
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            "-p", &host.port.to_string(),
            &format!("{}@{}", host.username, host.hostname),
            &format!("ls -la --time-style=+%s {}", path),
        ])
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::BAD_GATEWAY, format!("ls failed: {}", stderr)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.splitn(8, char::is_whitespace).filter(|s| !s.is_empty()).collect();
        if parts.len() >= 7 {
            let perms = parts[0];
            let size: u64 = parts[3].parse().unwrap_or(0);
            let name = parts.last().unwrap_or(&"");
            if *name == "." || *name == ".." { continue; }
            files.push(json!({
                "name": name,
                "is_dir": perms.starts_with('d'),
                "size": size,
                "perms": perms,
            }));
        }
    }

    Ok(Json(json!({ "path": path, "entries": files })))
}

/// GET /api/terminal/sftp/{host_id}/read
pub async fn read_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Query(q): Query<SftpQuery>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = super::hosts::get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let path = q.path.as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "path required".into()))?;

    let filename = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    let data = if host.is_local {
        std::fs::read(path)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?
    } else {
        let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
        let key_path_expanded = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());
        let output = tokio::process::Command::new("scp")
            .args([
                "-i", &key_path_expanded,
                "-o", "StrictHostKeyChecking=no",
                "-P", &host.port.to_string(),
                &format!("{}@{}:{}", host.username, host.hostname, path),
                "/dev/stdout",
            ])
            .output()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        output.stdout
    };

    Ok(axum::response::Response::builder()
        .header("Content-Type", "application/octet-stream")
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
        .body(axum::body::Body::from(data))
        .unwrap())
}

/// POST /api/terminal/sftp/{host_id}/upload
pub async fn upload_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Query(q): Query<SftpQuery>,
    mut multipart: Multipart,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = super::hosts::get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let dest_dir = q.path.as_deref().unwrap_or("/tmp");

    while let Some(field) = multipart.next_field().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))? {
        let filename = field.file_name().unwrap_or("upload").to_string();
        let data = field.bytes().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

        // Size check
        if data.len() > mgr.config.sftp_max_upload_mb * 1024 * 1024 {
            return Err((StatusCode::PAYLOAD_TOO_LARGE, format!("max {}MB", mgr.config.sftp_max_upload_mb)));
        }

        let dest_path = format!("{}/{}", dest_dir, filename);

        if host.is_local {
            std::fs::write(&dest_path, &data)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            // Write to temp file then scp
            let tmp = format!("/tmp/syntaur-upload-{}", uuid::Uuid::new_v4());
            std::fs::write(&tmp, &data)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
            let key_path_expanded = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());
            let output = tokio::process::Command::new("scp")
                .args([
                    "-i", &key_path_expanded,
                    "-o", "StrictHostKeyChecking=no",
                    "-P", &host.port.to_string(),
                    &tmp,
                    &format!("{}@{}:{}", host.username, host.hostname, dest_path),
                ])
                .output()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            let _ = std::fs::remove_file(&tmp);

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err((StatusCode::BAD_GATEWAY, format!("scp failed: {}", stderr)));
            }
        }

        return Ok(Json(json!({ "success": true, "path": dest_path })));
    }

    Err((StatusCode::BAD_REQUEST, "no file in upload".into()))
}

/// POST /api/terminal/sftp/{host_id}/mkdir
pub async fn mkdir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Query(q): Query<SftpQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = super::hosts::get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let path = q.path.as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "path required".into()))?;

    if host.is_local {
        std::fs::create_dir_all(path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    } else {
        let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
        let key_path_expanded = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());
        let output = tokio::process::Command::new("ssh")
            .args([
                "-i", &key_path_expanded,
                "-o", "StrictHostKeyChecking=no",
                "-p", &host.port.to_string(),
                &format!("{}@{}", host.username, host.hostname),
                &format!("mkdir -p {}", path),
            ])
            .output()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((StatusCode::BAD_GATEWAY, format!("mkdir failed: {}", stderr)));
        }
    }

    Ok(Json(json!({ "success": true, "path": path })))
}

/// DELETE /api/terminal/sftp/{host_id}/rm
pub async fn rm(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(host_id): axum::extract::Path<i64>,
    Query(q): Query<SftpQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mgr = state.terminal.as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "terminal module disabled".into()))?;

    let host = super::hosts::get_host_by_id(&mgr.db_path, host_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;

    let path = q.path.as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "path required".into()))?;

    // Safety: refuse to delete root-level paths
    if path.matches('/').count() < 2 {
        return Err((StatusCode::FORBIDDEN, "refusing to delete top-level path".into()));
    }

    if host.is_local {
        let meta = std::fs::metadata(path)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        if meta.is_dir() {
            std::fs::remove_dir_all(path)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        } else {
            std::fs::remove_file(path)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    } else {
        let key_path = host.private_key.as_deref().unwrap_or("~/.ssh/id_ed25519");
        let key_path_expanded = key_path.replace("~", &std::env::var("HOME").unwrap_or_default());
        let output = tokio::process::Command::new("ssh")
            .args([
                "-i", &key_path_expanded,
                "-o", "StrictHostKeyChecking=no",
                "-p", &host.port.to_string(),
                &format!("{}@{}", host.username, host.hostname),
                &format!("rm -rf {}", path),
            ])
            .output()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((StatusCode::BAD_GATEWAY, format!("rm failed: {}", stderr)));
        }
    }

    Ok(Json(json!({ "success": true })))
}
