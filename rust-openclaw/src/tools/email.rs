use log::{info, error};
use std::time::Duration;

struct EmailAccount {
    imap_host: &'static str,
    smtp_host: &'static str,
    smtp_port: u16,
    email: &'static str,
    password: String,
    display_name: &'static str,
}

fn get_accounts() -> Vec<(&'static str, EmailAccount)> {
    vec![
        ("crimson-lantern", EmailAccount {
            imap_host: "imap.gmail.com",
            smtp_host: "smtp.gmail.com",
            smtp_port: 465,
            email: "CrimsonLanternMusic@gmail.com",
            password: std::env::var("GMAIL_APP_PASSWORD").unwrap_or_else(|_| "dyiq rkoe zyqg qiaj".to_string()),
            display_name: "Crimson Lantern",
        }),
        ("felix", EmailAccount {
            imap_host: "outlook.office365.com",
            smtp_host: "smtp-mail.outlook.com",
            smtp_port: 587,
            email: "felixcherry1985@outlook.com",
            password: std::env::var("OUTLOOK_PASSWORD").unwrap_or_else(|_| "FelixCherry2026!".to_string()),
            display_name: "Felix Cherry",
        }),
    ]
}

fn get_account(name: &str) -> EmailAccount {
    let key = name.to_lowercase();
    let accounts = get_accounts();
    for (id, acc) in &accounts {
        if key == *id || key == acc.email.to_lowercase()
            || (key == "outlook" && acc.imap_host.contains("outlook"))
            || (key == "gmail" && acc.imap_host.contains("gmail"))
            || (key.is_empty() && *id == "crimson-lantern")
            || (key == "default" && *id == "crimson-lantern")
        {
            return EmailAccount {
                imap_host: acc.imap_host,
                smtp_host: acc.smtp_host,
                smtp_port: acc.smtp_port,
                email: acc.email,
                password: acc.password.clone(),
                display_name: acc.display_name,
            };
        }
    }
    get_accounts().remove(0).1 // default
}

/// Read recent emails from inbox
pub async fn email_read(folder: &str, count: usize) -> Result<String, String> {
    email_read_account(folder, count, "").await
}

/// Read recent emails from a specific account
pub async fn email_read_account(folder: &str, count: usize, account: &str) -> Result<String, String> {
    let folder = if folder.is_empty() { "INBOX".to_string() } else { folder.to_string() };
    let count = count.max(1).min(20);
    let acc = get_account(account);

    info!("[email] Reading {} emails from {} ({})", count, folder, acc.email);

    let imap_host = acc.imap_host.to_string();
    let email = acc.email.to_string();
    let password = acc.password.to_string();

    tokio::task::spawn_blocking(move || {
        email_read_sync(&folder, count, &imap_host, &email, &password)
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

fn email_read_sync(folder: &str, count: usize, imap_host: &str, email: &str, password: &str) -> Result<String, String> {
    use std::io::{Read, Write, BufRead, BufReader};
    use std::net::TcpStream;

    // Use native_tls for IMAP SSL
    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS error: {}", e))?;
    let stream = TcpStream::connect((imap_host, 993))
        .map_err(|e| format!("Connect error: {}", e))?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut tls = connector.connect(imap_host, stream)
        .map_err(|e| format!("TLS handshake error: {}", e))?;

    let mut buf = vec![0u8; 65536];

    // Read greeting
    let n = tls.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;

    // Login (password quoted because it may contain spaces)
    let cmd = format!("a1 LOGIN \"{}\" \"{}\"\r\n", email, password);
    tls.write_all(cmd.as_bytes()).map_err(|e| format!("Write error: {}", e))?;
    let n = tls.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;
    let resp = String::from_utf8_lossy(&buf[..n]);
    if !resp.contains("OK") {
        return Err(format!("Login failed: {}", resp));
    }

    // Select folder
    let cmd = format!("a2 SELECT {}\r\n", folder);
    tls.write_all(cmd.as_bytes()).map_err(|e| format!("Write error: {}", e))?;
    let n = tls.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;
    let resp = String::from_utf8_lossy(&buf[..n]);

    // Extract EXISTS count
    let total: usize = resp.lines()
        .find(|l| l.contains("EXISTS"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);

    if total == 0 {
        tls.write_all(b"a9 LOGOUT\r\n").ok();
        return Ok("No emails in folder.".to_string());
    }

    // Fetch last N emails
    let start = total.saturating_sub(count) + 1;
    let cmd = format!("a3 FETCH {}:{} (BODY[HEADER.FIELDS (FROM SUBJECT DATE)] BODY[TEXT])\r\n", start, total);
    tls.write_all(cmd.as_bytes()).map_err(|e| format!("Write error: {}", e))?;

    // Read response (may need multiple reads)
    let mut full_resp = String::new();
    loop {
        let n = tls.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        full_resp.push_str(&String::from_utf8_lossy(&buf[..n]));
        if full_resp.contains("a3 OK") || full_resp.contains("a3 NO") || full_resp.contains("a3 BAD") {
            break;
        }
    }

    // Logout
    tls.write_all(b"a9 LOGOUT\r\n").ok();

    // Parse emails simply
    let mut emails = Vec::new();
    let mut current_headers = String::new();
    let mut current_body = String::new();
    let mut in_body = false;

    for line in full_resp.lines() {
        if line.starts_with("From:") || line.starts_with("Subject:") || line.starts_with("Date:") {
            current_headers.push_str(line);
            current_headers.push('\n');
            in_body = false;
        } else if line.contains("BODY[TEXT]") {
            in_body = true;
        } else if in_body && !line.starts_with(")") && !line.starts_with("a3 ") {
            if current_body.len() < 500 {
                current_body.push_str(line);
                current_body.push('\n');
            }
        } else if line.starts_with(")") && !current_headers.is_empty() {
            emails.push(format!("{}\n{}", current_headers.trim(), current_body.trim()));
            current_headers.clear();
            current_body.clear();
            in_body = false;
        }
    }

    if emails.is_empty() {
        Ok(format!("Folder {} has {} emails but couldn't parse them.", folder, total))
    } else {
        Ok(format!("{} emails:\n\n{}", emails.len(), emails.join("\n\n---\n\n")))
    }
}

/// Send an email via SMTP
pub async fn email_send(to: &str, subject: &str, body: &str) -> Result<String, String> {
    email_send_account(to, subject, body, "").await
}

pub async fn email_send_account(to: &str, subject: &str, body: &str, account: &str) -> Result<String, String> {
    if to.is_empty() || subject.is_empty() || body.is_empty() {
        return Err("to, subject, and body are required".to_string());
    }

    let acc = get_account(account);
    info!("[email] Sending from {} to {} subject: {}", acc.email, to, subject);

    let to = to.to_string();
    let subject = subject.to_string();
    let body = body.to_string();
    let smtp_host = acc.smtp_host.to_string();
    let smtp_port = acc.smtp_port;
    let email = acc.email.to_string();
    let password = acc.password.to_string();
    let display_name = acc.display_name.to_string();

    tokio::task::spawn_blocking(move || {
        email_send_sync(&to, &subject, &body, &smtp_host, smtp_port, &email, &password, &display_name)
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

fn email_send_sync(to: &str, subject: &str, body: &str, smtp_host: &str, smtp_port: u16, email: &str, password: &str, display_name: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let connector = native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS error: {}", e))?;

    // Port 465 = implicit TLS, port 587 = STARTTLS
    let mut tls = if smtp_port == 465 {
        let stream = TcpStream::connect((smtp_host, 465))
            .map_err(|e| format!("Connect error: {}", e))?;
        stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
        connector.connect(smtp_host, stream)
            .map_err(|e| format!("TLS error: {}", e))?
    } else {
        // STARTTLS for port 587
        let mut stream = TcpStream::connect((smtp_host, 587))
            .map_err(|e| format!("Connect error: {}", e))?;
        stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
        let mut buf = vec![0u8; 4096];
        stream.read(&mut buf).ok(); // greeting
        stream.write_all(b"EHLO localhost\r\n").ok();
        stream.read(&mut buf).ok();
        stream.write_all(b"STARTTLS\r\n").ok();
        stream.read(&mut buf).ok();
        connector.connect(smtp_host, stream)
            .map_err(|e| format!("STARTTLS error: {}", e))?
    };

    let mut buf = vec![0u8; 4096];

    // Read greeting
    tls.read(&mut buf).ok();

    // EHLO
    tls.write_all(b"EHLO localhost\r\n").ok();
    tls.read(&mut buf).ok();

    // AUTH LOGIN
    tls.write_all(b"AUTH LOGIN\r\n").ok();
    tls.read(&mut buf).ok();

    // Username (base64)
    let user_b64 = base64_encode(email.as_bytes());
    tls.write_all(format!("{}\r\n", user_b64).as_bytes()).ok();
    tls.read(&mut buf).ok();

    // Password (base64)
    let pass_b64 = base64_encode(password.as_bytes());
    tls.write_all(format!("{}\r\n", pass_b64).as_bytes()).ok();
    let n = tls.read(&mut buf).unwrap_or(0);
    let resp = String::from_utf8_lossy(&buf[..n]);
    if !resp.starts_with("235") {
        return Err(format!("Auth failed: {}", resp));
    }

    // MAIL FROM
    tls.write_all(format!("MAIL FROM:<{}>\r\n", email).as_bytes()).ok();
    tls.read(&mut buf).ok();

    // RCPT TO
    tls.write_all(format!("RCPT TO:<{}>\r\n", to).as_bytes()).ok();
    tls.read(&mut buf).ok();

    // DATA
    tls.write_all(b"DATA\r\n").ok();
    tls.read(&mut buf).ok();

    // Message
    let message = format!(
        "From: {} <{}>\r\nTo: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{}\r\n.\r\n",
        display_name, email, to, subject, body
    );
    tls.write_all(message.as_bytes()).ok();
    let n = tls.read(&mut buf).unwrap_or(0);
    let resp = String::from_utf8_lossy(&buf[..n]);

    // QUIT
    tls.write_all(b"QUIT\r\n").ok();

    if resp.starts_with("250") {
        info!("[email] Sent to {}", to);
        Ok(format!("Email sent to {}", to))
    } else {
        Err(format!("Send failed: {}", resp))
    }
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 { result.push(CHARS[((n >> 6) & 63) as usize] as char); } else { result.push('='); }
        if chunk.len() > 2 { result.push(CHARS[(n & 63) as usize] as char); } else { result.push('='); }
    }
    result
}
