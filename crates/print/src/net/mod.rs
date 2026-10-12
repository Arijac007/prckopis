//! Printing straight to a network printer, without a system print queue.
//!
//! Offices often have printers nobody installed on the laptop in front of them. This module
//! talks to them directly over the three protocols business printers speak:
//!
//! - **IPP** (`ipp://host[:631]/path`, RFC 8010/8011): the job travels with its options (copies,
//!   two-sided, colour, held/PIN release, finishing), and the printer reports its state, its
//!   supply levels and each job's progress. Plain HTTP only; `ipps://` (TLS) is recognised and
//!   refused with a clear error.
//! - **LPD** (`lpd://host[:515]/queue`, RFC 1179): the classic queue protocol.
//! - **RAW / JetDirect / AppSocket** (`socket://host[:9100]`): the bytes go straight to the
//!   interpreter. For LPD and RAW the options travel as a PJL job header, which most office
//!   laser printers understand.
//!
//! [`discover`] finds printers on the local network with multicast DNS (DNS-SD), so nobody
//! needs to type an address. Every byte read from the network is untrusted: replies are capped
//! in size, parsed with bounds checks, and turned into errors rather than panics.

pub mod find;
pub mod http;
pub mod ipp;
pub mod lpd;
pub mod mdns;
pub mod pjl;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::PrintError;
pub use crate::spool::Duplex;

/// How long to wait for a printer to accept a connection, answer, or take more data.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// The protocol a network printer is reached with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// IPP over plain HTTP.
    Ipp,
    /// IPP over TLS; listed by discovery but not yet supported for jobs.
    Ipps,
    /// Line Printer Daemon.
    Lpd,
    /// A raw TCP socket (JetDirect / AppSocket).
    Raw,
}

impl Protocol {
    pub fn scheme(self) -> &'static str {
        match self {
            Protocol::Ipp => "ipp",
            Protocol::Ipps => "ipps",
            Protocol::Lpd => "lpd",
            Protocol::Raw => "socket",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ipp | Protocol::Ipps => 631,
            Protocol::Lpd => 515,
            Protocol::Raw => 9100,
        }
    }
}

/// Where a network printer is: a protocol, a host, a port and (IPP) a resource path or (LPD) a
/// queue name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub protocol: Protocol,
    /// A host name or an IP address (IPv6 without brackets).
    pub host: String,
    pub port: u16,
    /// IPP: the resource path, starting with `/`. LPD: the queue name. RAW: empty.
    pub path: String,
}

impl Target {
    /// Parse a printer address such as `ipp://10.0.0.5/ipp/print`, `lpd://printer/queue`,
    /// `socket://10.0.0.5:9100` (also `raw://`, `jetdirect://`, `http://` for IPP), or a bare
    /// `host[:port]`, which means IPP.
    pub fn parse(uri: &str) -> Result<Target, PrintError> {
        let uri = uri.trim();
        if uri.is_empty() || uri.len() > 1024 || uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(PrintError::Invalid("a printer address must be one line without spaces, at most 1024 characters".into()));
        }
        let (scheme, rest) = match uri.split_once("://") {
            Some((s, r)) => (s.to_ascii_lowercase(), r),
            None => ("ipp".to_string(), uri),
        };
        let protocol = match scheme.as_str() {
            "ipp" | "http" => Protocol::Ipp,
            "ipps" | "https" => Protocol::Ipps,
            "lpd" | "lpr" => Protocol::Lpd,
            "socket" | "raw" | "jetdirect" | "tcp" => Protocol::Raw,
            other => {
                return Err(PrintError::Invalid(format!("unknown printer address scheme {other:?}; use ipp://, lpd:// or socket:// (also raw://)")));
            }
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("")),
            None => (rest, ""),
        };
        // Drop user info: credentials are never sent in plain text by this client.
        let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
        let (host, port) = split_host_port(authority)?;
        if host.is_empty() {
            return Err(PrintError::Invalid(format!("{uri:?} has no host")));
        }
        if !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%')) {
            return Err(PrintError::Invalid(format!("{host:?} is not a host name or IP address")));
        }
        let port = port.unwrap_or(protocol.default_port());
        let path = match protocol {
            Protocol::Ipp | Protocol::Ipps => {
                if path.is_empty() || path == "/" {
                    "/ipp/print".to_string()
                } else {
                    path.to_string()
                }
            }
            Protocol::Lpd => {
                let queue = path.trim_start_matches('/');
                let queue = if queue.is_empty() { "lp" } else { queue };
                if queue.len() > 64 || !queue.bytes().all(|b| b.is_ascii_graphic() && b != b'/') {
                    return Err(PrintError::Invalid(format!("{queue:?} is not an LPD queue name")));
                }
                queue.to_string()
            }
            Protocol::Raw => String::new(),
        };
        Ok(Target { protocol, host: host.to_string(), port, path })
    }

    /// The address written back as a URI.
    pub fn uri(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let port = if self.port == self.protocol.default_port() { String::new() } else { format!(":{}", self.port) };
        match self.protocol {
            Protocol::Ipp | Protocol::Ipps => format!("{}://{host}{port}{}", self.protocol.scheme(), self.path),
            Protocol::Lpd => format!("lpd://{host}{port}/{}", self.path),
            Protocol::Raw => format!("socket://{host}{port}"),
        }
    }

    /// `host:port` for the HTTP Host header and messages.
    pub(crate) fn authority(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }

    /// Open a TCP connection, trying each resolved address in turn.
    pub(crate) fn connect(&self, timeout: Duration) -> Result<TcpStream, PrintError> {
        let addrs: Vec<SocketAddr> = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| PrintError::Spool(format!("cannot find printer {}: {e}", self.host)))?
            .take(8)
            .collect();
        let mut last = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(s) => {
                    s.set_read_timeout(Some(timeout)).map_err(io(self))?;
                    s.set_write_timeout(Some(timeout)).map_err(io(self))?;
                    let _ = s.set_nodelay(true);
                    return Ok(s);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(PrintError::Spool(match last {
            Some(e) => format!("cannot connect to printer {}: {e}", self.authority()),
            None => format!("cannot find printer {}", self.host),
        }))
    }
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), PrintError> {
    let parse_port = |p: &str| -> Result<Option<u16>, PrintError> {
        if p.is_empty() {
            return Ok(None);
        }
        match p.parse::<u16>() {
            Ok(0) | Err(_) => Err(PrintError::Invalid(format!("{p:?} is not a port number (1–65535)"))),
            Ok(n) => Ok(Some(n)),
        }
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| PrintError::Invalid(format!("{authority:?}: missing ']'")))?;
        let port = match after.strip_prefix(':') {
            Some(p) => parse_port(p)?,
            None if after.is_empty() => None,
            None => return Err(PrintError::Invalid(format!("{authority:?} is not host[:port]"))),
        };
        return Ok((host, port));
    }
    match authority.matches(':').count() {
        0 => Ok((authority, None)),
        1 => {
            let (h, p) = authority.split_once(':').unwrap_or((authority, ""));
            Ok((h, parse_port(p)?))
        }
        // A bare IPv6 address without brackets: no port.
        _ => Ok((authority, None)),
    }
}

pub(crate) fn io(t: &Target) -> impl Fn(std::io::Error) -> PrintError + '_ {
    move |e| PrintError::Spool(format!("printer {}: {e}", t.authority()))
}

/// Colour handling asked of the printer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// The printer's default.
    #[default]
    Auto,
    Color,
    Monochrome,
}

/// A finishing step the printer applies after printing (IPP `finishings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finishing {
    Staple,
    Punch,
    Fold,
}

impl Finishing {
    pub fn parse(s: &str) -> Option<Finishing> {
        match s {
            "staple" => Some(Finishing::Staple),
            "punch" => Some(Finishing::Punch),
            "fold" => Some(Finishing::Fold),
            _ => None,
        }
    }

    /// The IPP `finishings` enum value (RFC 8011 §5.2.6).
    pub fn ipp(self) -> i32 {
        match self {
            Finishing::Staple => 4,
            Finishing::Punch => 5,
            Finishing::Fold => 10,
        }
    }
}

/// A job sent to a network printer.
#[derive(Clone, Debug, PartialEq)]
pub struct NetJob {
    pub copies: u32,
    pub collate: bool,
    /// `None` keeps the printer's default.
    pub duplex: Option<Duplex>,
    pub color: ColorMode,
    /// The job's name in the printer's queue and its job log.
    pub title: String,
    /// Who the job is for: shown in the queue and used for accounting.
    pub user: String,
    /// Hold the job at the printer until someone releases it there (pull / secure printing).
    pub hold: bool,
    /// A PIN to release the job at the printer's panel (4 to 10 digits). Implies `hold`.
    pub pin: Option<String>,
    /// An accounting / cost-centre code, for printers that track usage per department.
    pub account: Option<String>,
    pub finishings: Vec<Finishing>,
    /// LPD and RAW: wrap the document in a PJL job header carrying the options above.
    pub pjl: bool,
}

impl Default for NetJob {
    fn default() -> Self {
        NetJob {
            copies: 1,
            collate: true,
            duplex: None,
            color: ColorMode::Auto,
            title: "Prckopis".into(),
            user: default_user(),
            hold: false,
            pin: None,
            account: None,
            finishings: Vec::new(),
            pjl: true,
        }
    }
}

impl NetJob {
    /// Check the job's values before anything goes on the wire.
    pub fn validate(&self) -> Result<(), PrintError> {
        if !(1..=999).contains(&self.copies) {
            return Err(PrintError::Invalid(format!("copies must be 1–999, not {}", self.copies)));
        }
        if let Some(pin) = &self.pin
            && !(4..=10).contains(&pin.len())
        {
            return Err(PrintError::Invalid("a release PIN must be 4 to 10 digits".into()));
        }
        if let Some(pin) = &self.pin
            && !pin.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(PrintError::Invalid("a release PIN must be digits only".into()));
        }
        Ok(())
    }

    pub(crate) fn held(&self) -> bool {
        self.hold || self.pin.is_some()
    }
}

/// The user name of whoever runs the app, for the printer's job log.
pub fn default_user() -> String {
    let name = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let name = clean_text(&name, 64);
    if name.is_empty() { "prckopis".into() } else { name }
}

/// Keep printable characters only (no quotes or control characters), at most `max` of them.
pub(crate) fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '\\')).take(max).collect::<String>().trim().to_string()
}

/// What a printer accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub protocol: Protocol,
    /// IPP's job id; for LPD the job number sent; none for RAW.
    pub job_id: Option<i32>,
    /// IPP's job URI, when the printer reported one.
    pub job_uri: Option<String>,
    /// IPP's job state after submission (e.g. `pending-held`), when reported.
    pub state: Option<String>,
    /// Options the printer said it ignored or changed.
    pub ignored: Vec<String>,
    pub bytes: usize,
}

/// Send a print-ready PDF to a network printer.
pub fn submit(target: &Target, pdf: &[u8], job: &NetJob) -> Result<Receipt, PrintError> {
    submit_with(target, pdf, job, DEFAULT_TIMEOUT)
}

pub fn submit_with(target: &Target, pdf: &[u8], job: &NetJob, timeout: Duration) -> Result<Receipt, PrintError> {
    job.validate()?;
    if pdf.is_empty() {
        return Err(PrintError::NoPages);
    }
    match target.protocol {
        Protocol::Ipp => ipp::print_job(target, pdf, job, timeout),
        Protocol::Ipps => Err(ipps_unsupported(target)),
        Protocol::Lpd => lpd::print(target, pdf, job, timeout),
        Protocol::Raw => {
            let data = if job.pjl { pjl::wrap(pdf, job) } else { pdf.to_vec() };
            let mut s = target.connect(timeout)?;
            s.write_all(&data).map_err(io(target))?;
            s.flush().map_err(io(target))?;
            let _ = s.shutdown(std::net::Shutdown::Write);
            // Some printers send a status line back; drain a little so the connection closes
            // cleanly, but never wait on it.
            let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
            let mut sink = [0u8; 512];
            let _ = s.read(&mut sink);
            Ok(Receipt { protocol: Protocol::Raw, job_id: None, job_uri: None, state: None, ignored: Vec::new(), bytes: data.len() })
        }
    }
}

pub(crate) fn ipps_unsupported(t: &Target) -> PrintError {
    PrintError::Spool(format!(
        "{} uses IPP over TLS (ipps), which direct printing does not support yet; try ipp://{}/… on the same printer, \
         or add it as a system printer",
        t.uri(),
        t.host
    ))
}

/// A printer's state, as IPP reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrinterStatus {
    pub name: Option<String>,
    pub make_and_model: Option<String>,
    pub location: Option<String>,
    /// `idle`, `processing` or `stopped`.
    pub state: Option<String>,
    pub state_reasons: Vec<String>,
    pub state_message: Option<String>,
    pub accepting_jobs: Option<bool>,
    pub queued_jobs: Option<i32>,
    pub document_formats: Vec<String>,
    pub sides: Vec<String>,
    pub color: Option<bool>,
    /// Toner / ink / supply levels: name and percent (negative when unknown).
    pub supplies: Vec<(String, i32)>,
}

impl PrinterStatus {
    /// Whether the printer takes PDF as it is (or sniffs the format itself).
    pub fn takes_pdf(&self) -> bool {
        self.document_formats.is_empty()
            || self.document_formats.iter().any(|f| f.eq_ignore_ascii_case("application/pdf") || f.eq_ignore_ascii_case("application/octet-stream"))
    }
}

/// Ask an IPP printer how it is (state, supplies, what it accepts).
pub fn status(target: &Target) -> Result<PrinterStatus, PrintError> {
    match target.protocol {
        Protocol::Ipp => ipp::printer_status(target, DEFAULT_TIMEOUT),
        Protocol::Ipps => Err(ipps_unsupported(target)),
        Protocol::Lpd => lpd::status(target, DEFAULT_TIMEOUT),
        Protocol::Raw => {
            // A raw socket has no status protocol; all we can say is whether it answers.
            target.connect(DEFAULT_TIMEOUT)?;
            Ok(PrinterStatus { state: Some("reachable".into()), ..Default::default() })
        }
    }
}

/// A job's progress at an IPP printer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobStatus {
    pub job_id: i32,
    pub name: Option<String>,
    pub user: Option<String>,
    /// `pending`, `pending-held`, `processing`, `processing-stopped`, `canceled`, `aborted`,
    /// `completed`.
    pub state: Option<String>,
    pub state_reasons: Vec<String>,
    pub sheets_completed: Option<i32>,
}

pub fn job_status(target: &Target, job_id: i32) -> Result<JobStatus, PrintError> {
    ipp_only(target)?;
    ipp::job_status(target, job_id, DEFAULT_TIMEOUT)
}

/// The printer's jobs: the ones still to print, or (`completed`) the finished ones.
pub fn jobs(target: &Target, completed: bool, mine_only: Option<&str>) -> Result<Vec<JobStatus>, PrintError> {
    ipp_only(target)?;
    ipp::jobs(target, completed, mine_only, DEFAULT_TIMEOUT)
}

pub fn cancel_job(target: &Target, job_id: i32, user: &str) -> Result<(), PrintError> {
    ipp_only(target)?;
    ipp::simple_job_op(target, ipp::op::CANCEL_JOB, job_id, user, DEFAULT_TIMEOUT)
}

/// Release a held job so it prints now.
pub fn release_job(target: &Target, job_id: i32, user: &str) -> Result<(), PrintError> {
    ipp_only(target)?;
    ipp::simple_job_op(target, ipp::op::RELEASE_JOB, job_id, user, DEFAULT_TIMEOUT)
}

fn ipp_only(target: &Target) -> Result<(), PrintError> {
    match target.protocol {
        Protocol::Ipp => Ok(()),
        Protocol::Ipps => Err(ipps_unsupported(target)),
        _ => Err(PrintError::Invalid(format!("{} has no job control; only IPP printers report and manage jobs", target.uri()))),
    }
}

pub use find::{SearchOptions, parse_range, search};
pub use mdns::{Found, discover, discover_with};
#[cfg(test)]
mod tests;
