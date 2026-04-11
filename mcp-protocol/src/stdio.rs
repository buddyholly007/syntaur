//! Newline-delimited JSON-RPC stdio framing.
//!
//! Both ends of an MCP stdio session use this: the client wraps a child
//! process's stdin/stdout, the server wraps its own stdin/stdout. The frame
//! format is one JSON object per line, terminated by `\n`.

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::McpError;

/// Read one JSON-encoded line from `reader` and parse it. Returns `Ok(None)`
/// when the stream is at EOF, `Ok(Some(line))` for a non-empty line.
///
/// We return the raw line as a `String` so callers can deserialize into the
/// concrete type they want without an extra round-trip through `Value`.
pub async fn read_line<R>(reader: &mut BufReader<R>) -> Result<Option<String>, McpError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let mut buf = String::new();
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = buf.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            // Some clients send empty lines; just skip them.
            continue;
        }
        return Ok(Some(trimmed.to_string()));
    }
}

/// Serialize a value to JSON and write it to `writer` followed by `\n`.
/// Flushes after each frame so the peer doesn't get stuck waiting on a
/// half-buffered request.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), McpError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let line = serde_json::to_string(value)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
