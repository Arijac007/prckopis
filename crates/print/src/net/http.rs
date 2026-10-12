//! Just enough HTTP/1.1 to carry IPP: one POST per connection, a body sized by Content-Length,
//! and a reply read by Content-Length, chunked encoding or until the printer closes. Headers
//! and bodies from the network are capped.

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use super::{Target, io};
use crate::PrintError;

/// Reply headers larger than this are refused.
const MAX_HEADER: usize = 64 * 1024;
/// Reply bodies larger than this are refused (IPP replies are small; attributes of a big printer
/// fit in well under a megabyte).
pub const MAX_BODY: usize = 16 * 1024 * 1024;

/// POST `body` to the target's path and return the reply body of a 2xx response.
pub fn post(target: &Target, content_type: &str, body: &[u8], timeout: Duration) -> Result<Vec<u8>, PrintError> {
    let mut s = target.connect(timeout)?;
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: PdfCraft\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path_of(target),
        target.authority(),
        body.len()
    );
    s.write_all(head.as_bytes()).map_err(io(target))?;
    s.write_all(body).map_err(io(target))?;
    s.flush().map_err(io(target))?;
    read_response(&mut BufReader::new(s), target)
}

fn path_of(target: &Target) -> String {
    // Only characters that are safe in a request line go out; anything else is percent-encoded.
    let mut out = String::with_capacity(target.path.len());
    for b in target.path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~%!$&'()*+,;=:@?".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    if out.starts_with('/') { out } else { format!("/{out}") }
}

/// Read one HTTP response: status line, headers, body.
pub fn read_response<R: BufRead>(r: &mut R, target: &Target) -> Result<Vec<u8>, PrintError> {
    loop {
        let (code, headers) = read_head(r, target)?;
        // Interim responses (100 Continue) carry no body; the real one follows.
        if (100..200).contains(&code) {
            continue;
        }
        let header = |name: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        if !(200..300).contains(&code) {
            return Err(PrintError::Spool(match code {
                401 | 403 => format!("printer {} needs a sign-in (HTTP {code}); add it as a system printer instead", target.authority()),
                404 => format!("printer {} has no IPP service at {} (HTTP 404); check the address's path", target.authority(), target.path),
                426 => super::ipps_unsupported(target).to_string(),
                _ => format!("printer {} answered HTTP {code}", target.authority()),
            }));
        }
        let chunked = header("Transfer-Encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let body = if chunked {
            read_chunked(r, target)?
        } else if let Some(len) = header("Content-Length") {
            let len: usize = len.trim().parse().map_err(|_| bad(target, "a bad Content-Length"))?;
            if len > MAX_BODY {
                return Err(bad(target, "a reply that is too large"));
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).map_err(io(target))?;
            body
        } else {
            let mut body = Vec::new();
            r.take(MAX_BODY as u64 + 1).read_to_end(&mut body).map_err(io(target))?;
            if body.len() > MAX_BODY {
                return Err(bad(target, "a reply that is too large"));
            }
            body
        };
        return Ok(body);
    }
}

fn bad(target: &Target, what: &str) -> PrintError {
    PrintError::Spool(format!("printer {} sent {what}", target.authority()))
}

fn read_line<R: BufRead>(r: &mut R, target: &Target, budget: &mut usize) -> Result<String, PrintError> {
    let mut line = Vec::new();
    let n = r.take((*budget as u64).saturating_add(1)).read_until(b'\n', &mut line).map_err(io(target))?;
    if n == 0 {
        return Err(bad(target, "no reply (the connection closed)"));
    }
    if n > *budget {
        return Err(bad(target, "headers that are too long"));
    }
    *budget -= n;
    Ok(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string())
}

fn read_head<R: BufRead>(r: &mut R, target: &Target) -> Result<(u16, Vec<(String, String)>), PrintError> {
    let mut budget = MAX_HEADER;
    let status = read_line(r, target, &mut budget)?;
    let mut parts = status.split_whitespace();
    let proto = parts.next().unwrap_or("");
    if !proto.starts_with("HTTP/") {
        return Err(bad(target, "something that is not an HTTP reply (is this an IPP address?)"));
    }
    let code: u16 = parts.next().and_then(|c| c.parse().ok()).ok_or_else(|| bad(target, "a bad HTTP status line"))?;
    let mut headers = Vec::new();
    loop {
        let line = read_line(r, target, &mut budget)?;
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && headers.len() < 200
        {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok((code, headers))
}

fn read_chunked<R: BufRead>(r: &mut R, target: &Target) -> Result<Vec<u8>, PrintError> {
    let mut body = Vec::new();
    loop {
        let mut budget = 1024;
        let line = read_line(r, target, &mut budget)?;
        let size = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| bad(target, "a bad chunk size"))?;
        if size == 0 {
            // Trailers until an empty line; tolerate a printer that just closes.
            let mut budget = MAX_HEADER;
            while let Ok(l) = read_line(r, target, &mut budget) {
                if l.is_empty() {
                    break;
                }
            }
            return Ok(body);
        }
        if body.len().saturating_add(size) > MAX_BODY {
            return Err(bad(target, "a reply that is too large"));
        }
        let start = body.len();
        body.resize(start + size, 0);
        r.read_exact(body.get_mut(start..).unwrap_or_default()).map_err(io(target))?;
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf).map_err(io(target))?;
    }
}
