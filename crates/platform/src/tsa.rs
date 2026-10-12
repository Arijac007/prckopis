//! The transport for RFC 3161 timestamp servers (Acrobat: Preferences ▸ Security ▸ Timestamp
//! Servers): one HTTP POST of a DER `TimeStampReq` (`application/timestamp-query`), the DER
//! `TimeStampResp` back. The reply is checked by `prckopis-sign` (imprint, nonce, the TSA's
//! signature), so plain HTTP is safe; that is what most public authorities serve. HTTPS
//! authorities are not reached yet: the error says so and suggests their http address.
//!
//! Everything from the network is untrusted: headers and bodies are capped, redirects are
//! followed at most [`MAX_REDIRECTS`] times, and only to other http addresses.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// How long one request may take to connect, and then to answer.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// Redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 3;
/// Reply bodies larger than this are refused (a token with its certificate chain is a few KB).
pub const MAX_REPLY: usize = 4 * 1024 * 1024;
const MAX_HEADER: usize = 64 * 1024;
const MAX_URL: usize = 2048;
const MAX_CREDENTIAL: usize = 256;

/// A configured timestamp authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimestampServer {
    /// What the user calls it ("DigiCert").
    pub name: String,
    pub url: String,
    /// HTTP Basic credentials, for authorities that need a sign-in.
    pub username: Option<String>,
    pub password: Option<String>,
}

impl TimestampServer {
    /// A server at `url`, named after its host.
    pub fn new(url: &str) -> Result<Self, String> {
        let parsed = parse_url(url)?;
        Ok(Self { name: parsed.host.clone(), url: url.trim().to_string(), username: None, password: None })
    }

    /// Check the address and the credentials.
    pub fn validate(&self) -> Result<(), String> {
        parse_url(&self.url)?;
        for (what, v) in [("user name", &self.username), ("password", &self.password)] {
            if let Some(v) = v
                && (v.len() > MAX_CREDENTIAL || v.chars().any(char::is_control))
            {
                return Err(format!("the timestamp server's {what} is too long or has control characters"));
            }
        }
        if self.username.as_deref().is_some_and(|u| u.contains(':')) {
            return Err("the timestamp server's user name can't contain a colon".into());
        }
        Ok(())
    }
}

/// An `http://host[:port]/path` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpUrl {
    pub host: String,
    pub port: u16,
    /// Starts with `/`; query included; only request-line-safe characters.
    pub path: String,
}

impl HttpUrl {
    fn authority(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == 80 { host } else { format!("{host}:{}", self.port) }
    }
}

/// Parse a timestamp server address. Only `http://` is reachable; `https://` is refused with
/// advice, anything else as not an address.
pub fn parse_url(url: &str) -> Result<HttpUrl, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("enter the timestamp server's address (for example http://timestamp.digicert.com)".into());
    }
    if url.len() > MAX_URL {
        return Err("the timestamp server's address is too long".into());
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err(format!(
            "{url}: HTTPS timestamp servers can't be reached yet. Most authorities also answer at the same address with http:// — timestamp replies are signed, so plain HTTP is safe"
        ));
    }
    let rest = if lower.starts_with("http://") { url.get(7..).unwrap_or("") } else { return Err(format!("{url} is not an http:// address")) };
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("")),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("put the user name and password in their own fields, not in the address".into());
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or_else(|| format!("{url}: unclosed [ in the address"))?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return Err(format!("{url} is not a valid address")),
        };
        (h.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), None),
        }
    };
    let port = match port {
        Some(p) => p.parse::<u16>().ok().filter(|&p| p != 0).ok_or_else(|| format!("{url}: the port must be 1–65535"))?,
        None => 80,
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_')) {
        return Err(format!("{url} has no valid host name"));
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    let mut safe = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~%!$&'()*+,;=:@?".contains(&b) {
            safe.push(char::from(b));
        } else {
            safe.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(HttpUrl { host: host.to_ascii_lowercase(), port, path: safe })
}

/// Send a DER `TimeStampReq` to `server` and return the DER `TimeStampResp` body.
pub fn post_query(server: &TimestampServer, query: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    server.validate()?;
    let auth = match (&server.username, &server.password) {
        (Some(u), p) if !u.is_empty() => Some(base64(format!("{u}:{}", p.as_deref().unwrap_or("")).as_bytes())),
        _ => None,
    };
    let mut url = parse_url(&server.url)?;
    for _ in 0..=MAX_REDIRECTS {
        match exchange(&url, auth.as_deref(), query, timeout)? {
            Reply::Body(body) => return Ok(body),
            Reply::Redirect(to) => {
                let next = if to.starts_with('/') { format!("http://{}{to}", url.authority()) } else { to };
                url = parse_url(&next).map_err(|e| format!("the timestamp server {} redirected: {e}", server.url))?;
            }
        }
    }
    Err(format!("the timestamp server {} redirected too many times", server.url))
}

enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

fn exchange(url: &HttpUrl, auth: Option<&str>, query: &[u8], timeout: Duration) -> Result<Reply, String> {
    let name = url.authority();
    let unreachable = |e: std::io::Error| format!("couldn't reach the timestamp server {name}: {e}");
    let addrs: Vec<_> = (url.host.as_str(), url.port).to_socket_addrs().map_err(unreachable)?.take(8).collect();
    let mut last = None;
    let mut stream = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut s = stream.ok_or_else(|| unreachable(last.unwrap_or_else(|| std::io::Error::other("no address"))))?;
    s.set_read_timeout(Some(timeout)).map_err(unreachable)?;
    s.set_write_timeout(Some(timeout)).map_err(unreachable)?;
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {name}\r\nUser-Agent: Prckopis\r\nContent-Type: application/timestamp-query\r\nAccept: application/timestamp-reply\r\nContent-Length: {}\r\nConnection: close\r\n",
        url.path,
        query.len()
    );
    if let Some(a) = auth {
        head.push_str(&format!("Authorization: Basic {a}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).map_err(unreachable)?;
    s.write_all(query).map_err(unreachable)?;
    s.flush().map_err(unreachable)?;
    read_reply(&mut BufReader::new(s), &name)
}

fn read_reply<R: BufRead>(r: &mut R, name: &str) -> Result<Reply, String> {
    let broken = |e: std::io::Error| format!("the timestamp server {name} broke off its answer: {e}");
    loop {
        let mut seen = 0usize;
        let mut line = |r: &mut R| -> Result<String, String> {
            let mut buf = Vec::new();
            let n = r.by_ref().take((MAX_HEADER - seen.min(MAX_HEADER)) as u64 + 1).read_until(b'\n', &mut buf).map_err(broken)?;
            seen = seen.saturating_add(n);
            if seen > MAX_HEADER {
                return Err(format!("the timestamp server {name} sent oversized headers"));
            }
            Ok(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string())
        };
        let status = line(r)?;
        if status.is_empty() {
            return Err(format!("the timestamp server {name} closed the connection without answering"));
        }
        let code: u16 = status
            .strip_prefix("HTTP/1.")
            .and_then(|s| s.get(2..))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| format!("the timestamp server {name} didn't answer in HTTP"))?;
        let mut headers = Vec::new();
        loop {
            let h = line(r)?;
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
        }
        let header = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        if (100..200).contains(&code) {
            continue;
        }
        if matches!(code, 301 | 302 | 303 | 307 | 308) {
            return header("location")
                .map(|l| Reply::Redirect(l.to_string()))
                .ok_or_else(|| format!("the timestamp server {name} redirected nowhere"));
        }
        if !(200..300).contains(&code) {
            return Err(match code {
                401 | 403 => format!("the timestamp server {name} needs a user name and password (HTTP {code})"),
                404 => format!("the timestamp server {name} has nothing at this address (HTTP 404); check its path"),
                _ => format!("the timestamp server {name} answered HTTP {code}"),
            });
        }
        let chunked = header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
        let body = if chunked {
            read_chunked(r, name)?
        } else if let Some(len) = header("content-length") {
            let len: usize = len.parse().map_err(|_| format!("the timestamp server {name} sent a bad Content-Length"))?;
            if len > MAX_REPLY {
                return Err(format!("the timestamp server {name} sent an oversized answer"));
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).map_err(broken)?;
            body
        } else {
            let mut body = Vec::new();
            r.by_ref().take(MAX_REPLY as u64 + 1).read_to_end(&mut body).map_err(broken)?;
            body
        };
        if body.len() > MAX_REPLY {
            return Err(format!("the timestamp server {name} sent an oversized answer"));
        }
        if header("content-type").is_some_and(|t| t.to_ascii_lowercase().starts_with("text/html")) {
            return Err(format!("the timestamp server {name} answered with a web page, not a timestamp; check its address"));
        }
        return Ok(Reply::Body(body));
    }
}

fn read_chunked<R: BufRead>(r: &mut R, name: &str) -> Result<Vec<u8>, String> {
    let broken = |e: std::io::Error| format!("the timestamp server {name} broke off its answer: {e}");
    let mut body = Vec::new();
    loop {
        let mut size = Vec::new();
        r.by_ref().take(64).read_until(b'\n', &mut size).map_err(broken)?;
        let text = String::from_utf8_lossy(&size);
        let hex = text.trim().split(';').next().unwrap_or("");
        let n = usize::from_str_radix(hex, 16).map_err(|_| format!("the timestamp server {name} sent a bad chunk"))?;
        if n == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(n) > MAX_REPLY {
            return Err(format!("the timestamp server {name} sent an oversized answer"));
        }
        let start = body.len();
        body.resize(start + n, 0);
        r.read_exact(body.get_mut(start..).unwrap_or_default()).map_err(broken)?;
        let mut crlf = Vec::new();
        r.by_ref().take(2).read_until(b'\n', &mut crlf).map_err(broken)?;
    }
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                // `n >> shift & 63` is below 64, a valid index.
                out.push(char::from(A.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn addresses() {
        let u = parse_url(" http://timestamp.digicert.com ").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("timestamp.digicert.com", 80, "/"));
        let u = parse_url("http://TSA.example:8080/tsr?x=1 y").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("tsa.example", 8080, "/tsr?x=1%20y"));
        assert_eq!(parse_url("http://[::1]:3180/").unwrap().authority(), "[::1]:3180");
        for bad in [
            "",
            "ftp://x",
            "timestamp.digicert.com",
            "http://",
            "http://a b/",
            "http://h:0/",
            "http://h:99999/",
            "http://u:p@h/",
            "http://h\r\nX: y/",
        ] {
            assert!(parse_url(bad).is_err(), "{bad:?}");
        }
        assert!(parse_url("https://freetsa.org/tsr").unwrap_err().contains("http://"));
    }

    #[test]
    fn credentials_are_checked() {
        let mut s = TimestampServer::new("http://tsa.example/").unwrap();
        assert_eq!(s.name, "tsa.example");
        s.username = Some("a:b".into());
        assert!(s.validate().is_err());
        s.username = Some("ana".into());
        s.password = Some("x\ny".into());
        assert!(s.validate().is_err());
    }

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
            ("Aladdin:open sesame", "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    /// Serve `replies` in order, one per connection; return what each request sent.
    fn server(replies: Vec<Vec<u8>>) -> (u16, thread::JoinHandle<Vec<Vec<u8>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s);
                let mut req = Vec::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    req.extend_from_slice(line.as_bytes());
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                req.extend_from_slice(&body);
                seen.push(req);
                r.into_inner().write_all(&reply).unwrap();
            }
            seen
        });
        (port, h)
    }

    #[test]
    fn a_query_is_posted_and_the_reply_returned() {
        let (port, h) = server(vec![b"HTTP/1.1 200 OK\r\nContent-Type: application/timestamp-reply\r\nContent-Length: 3\r\n\r\nabc".to_vec()]);
        let mut s = TimestampServer::new(&format!("http://127.0.0.1:{port}/tsa")).unwrap();
        s.username = Some("Aladdin".into());
        s.password = Some("open sesame".into());
        assert_eq!(post_query(&s, b"QUERY", DEFAULT_TIMEOUT).unwrap(), b"abc");
        let req = String::from_utf8(h.join().unwrap().remove(0)).unwrap();
        assert!(req.starts_with("POST /tsa HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("Content-Type: application/timestamp-query\r\n"));
        assert!(req.contains("Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n"));
        assert!(req.ends_with("\r\n\r\nQUERY"));
    }

    #[test]
    fn chunked_replies_redirects_and_errors() {
        let (port, h) = server(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n1;x\r\nc\r\n0\r\n\r\n".to_vec(),
        ]);
        let s = TimestampServer::new(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert_eq!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap(), b"abc");
        let reqs = h.join().unwrap();
        assert!(String::from_utf8_lossy(&reqs[1]).starts_with("POST /next "));

        let (port, h) = server(vec![
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\nhi".to_vec(),
            b"HTTP/1.1 301 Moved\r\nLocation: https://elsewhere/\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 99999999\r\n\r\n".to_vec(),
            b"garbage\r\n\r\n".to_vec(),
        ]);
        let s = TimestampServer::new(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap_err().contains("user name and password"));
        assert!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap_err().contains("web page"));
        assert!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap_err().contains("HTTPS"));
        assert!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap_err().contains("oversized"));
        assert!(post_query(&s, b"Q", DEFAULT_TIMEOUT).unwrap_err().contains("HTTP"));
        h.join().unwrap();
    }

    #[test]
    fn nobody_listening() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let s = TimestampServer::new(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(post_query(&s, b"Q", Duration::from_secs(2)).unwrap_err().contains("couldn't reach"));
    }
}
