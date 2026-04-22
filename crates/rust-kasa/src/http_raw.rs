//! Raw HTTP/1.1 client over TCP for talking to TP-Link's SHIP 2.0 server.
//!
//! Why not reqwest/hyper: TP-Link's embedded HTTP parser is case-sensitive
//! on header names — `Host:` works, `host:` returns HTTP 400. hyper
//! (reqwest's underlying client) lowercases all headers for HTTP/2
//! compatibility, which breaks this class of old devices. We emit
//! requests by hand with exactly the casing and ordering curl uses.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::KasaError;

const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed response: status, headers (case-preserving key/value), body bytes.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }

    pub fn headers_all(&self, name: &str) -> impl Iterator<Item = &str> {
        let lower = name.to_ascii_lowercase();
        self.headers.iter().filter_map(move |(k, v)| {
            if k.to_ascii_lowercase() == lower {
                Some(v.as_str())
            } else {
                None
            }
        })
    }
}

/// POST `path` to `host:80` with the given body, return the parsed response.
///
/// Headers emitted match curl byte-for-byte (title-case, same order):
///   Host: <host>
///   User-Agent: rust-kasa/0.1
///   Accept: */*
///   [Cookie: <cookie>]                  if provided
///   Content-Type: application/octet-stream
///   Content-Length: <len>
///
/// Only HTTP/1.1 with Content-Length body framing; no chunked, no
/// keep-alive — each call does a fresh TCP connect + `Connection: close`.
pub async fn post(
    host: &str,
    path: &str,
    body: &[u8],
    cookie: Option<&str>,
) -> Result<Response, KasaError> {
    let mut req = Vec::with_capacity(256 + body.len());
    req.extend_from_slice(format!("POST {path} HTTP/1.1\r\n").as_bytes());
    req.extend_from_slice(format!("Host: {host}\r\n").as_bytes());
    req.extend_from_slice(b"User-Agent: rust-kasa/0.1\r\n");
    req.extend_from_slice(b"Accept: */*\r\n");
    if let Some(c) = cookie {
        req.extend_from_slice(format!("Cookie: {c}\r\n").as_bytes());
    }
    req.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
    req.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    req.extend_from_slice(b"Connection: close\r\n\r\n");
    req.extend_from_slice(body);

    let addr = format!("{host}:80");
    let mut stream = timeout(IO_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "tcp connect",
        )))??;
    stream.set_nodelay(true)?;
    timeout(IO_TIMEOUT, stream.write_all(&req))
        .await
        .map_err(|_| KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "http write",
        )))??;

    // Read headers first (until \r\n\r\n), then exactly Content-Length
    // bytes of body. TP-Link's SHIP 2.0 server ignores the Connection:
    // close hint and keeps the socket open, so EOF-based reads hang.
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    let mut header_end: Option<usize> = None;
    while header_end.is_none() {
        let n = match timeout(IO_TIMEOUT, stream.read(&mut tmp)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(KasaError::Io(e)),
            Err(_) => {
                return Err(KasaError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "http header read",
                )))
            }
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        header_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    }
    let hdr_end = header_end.ok_or_else(|| {
        KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no header terminator",
        ))
    })?;

    // Parse out Content-Length so we know how much more to read.
    let head_str = std::str::from_utf8(&buf[..hdr_end]).map_err(|_| {
        KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "non-utf8 headers",
        ))
    })?;
    let mut content_length: usize = 0;
    for line in head_str.split("\r\n") {
        if let Some(rest) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = rest.trim().parse().unwrap_or(0);
            break;
        }
    }

    let have = buf.len() - hdr_end;
    let need = content_length.saturating_sub(have);
    if need > 0 {
        let mut remaining = need;
        while remaining > 0 {
            let n = match timeout(IO_TIMEOUT, stream.read(&mut tmp)).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(KasaError::Io(e)),
                Err(_) => {
                    return Err(KasaError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "http body read",
                    )))
                }
            };
            if n == 0 {
                break;
            }
            let take = n.min(remaining);
            buf.extend_from_slice(&tmp[..take]);
            remaining = remaining.saturating_sub(take);
        }
    }
    // Truncate any over-read (if server ignores Content-Length framing).
    buf.truncate(hdr_end + content_length);

    parse_response(&buf)
}

fn parse_response(data: &[u8]) -> Result<Response, KasaError> {
    // Find end of headers (double CRLF).
    let sep = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing \\r\\n\\r\\n header terminator",
        )))?;
    let head = &data[..sep];
    let body = data[sep + 4..].to_vec();

    let mut lines = head.split(|b| *b == b'\n').map(|l| {
        // strip trailing \r
        if l.last() == Some(&b'\r') {
            &l[..l.len() - 1]
        } else {
            l
        }
    });
    let status_line = lines
        .next()
        .ok_or(KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty status line",
        )))?;
    // HTTP/1.1 <code> <reason>
    let status = {
        let s = std::str::from_utf8(status_line).map_err(|_| {
            KasaError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-utf8 status line",
            ))
        })?;
        let mut parts = s.splitn(3, ' ');
        parts.next(); // "HTTP/1.1"
        let code = parts.next().ok_or(KasaError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad status line",
        )))?;
        code.parse::<u16>().map_err(|_| {
            KasaError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-numeric status",
            ))
        })?
    };

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(colon) = line.iter().position(|b| *b == b':') {
            let k = std::str::from_utf8(&line[..colon])
                .map_err(|_| {
                    KasaError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "non-utf8 header key",
                    ))
                })?
                .to_string();
            let v_start = colon + 1;
            let v = std::str::from_utf8(&line[v_start..])
                .map_err(|_| {
                    KasaError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "non-utf8 header value",
                    ))
                })?
                .trim()
                .to_string();
            headers.push((k, v));
        }
    }

    Ok(Response {
        status,
        headers,
        body,
    })
}
