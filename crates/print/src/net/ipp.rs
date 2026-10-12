//! The Internet Printing Protocol: the binary message encoding (RFC 8010 §3) and the handful of
//! operations a direct-printing client needs (RFC 8011 §4): Print-Job, Get-Printer-Attributes,
//! Get-Job-Attributes, Get-Jobs, Cancel-Job and Release-Job.
//!
//! Messages travel as the body of an HTTP POST with `Content-Type: application/ipp`.
//! Replies come from the network and are parsed defensively: every length is checked against
//! what is left, the number of attributes is capped, and nested collections are skipped with a
//! depth limit.

use std::time::Duration;

use super::{ColorMode, JobStatus, NetJob, PrinterStatus, Protocol, Receipt, Target, clean_text, http};
use crate::PrintError;
use crate::spool::Duplex;

/// Operation ids (RFC 8011 §5.4.15, PWG 5100.x).
pub mod op {
    pub const PRINT_JOB: u16 = 0x0002;
    pub const CANCEL_JOB: u16 = 0x0008;
    pub const GET_JOB_ATTRIBUTES: u16 = 0x0009;
    pub const GET_JOBS: u16 = 0x000A;
    pub const GET_PRINTER_ATTRIBUTES: u16 = 0x000B;
    pub const RELEASE_JOB: u16 = 0x000D;
}

/// Delimiter tags.
pub mod tag {
    pub const OPERATION: u8 = 0x01;
    pub const JOB: u8 = 0x02;
    pub const END: u8 = 0x03;
    pub const PRINTER: u8 = 0x04;
    pub const UNSUPPORTED: u8 = 0x05;

    pub const INTEGER: u8 = 0x21;
    pub const BOOLEAN: u8 = 0x22;
    pub const ENUM: u8 = 0x23;
    pub const OCTET_STRING: u8 = 0x30;
    pub const BEGIN_COLLECTION: u8 = 0x34;
    pub const TEXT_WITH_LANGUAGE: u8 = 0x35;
    pub const NAME_WITH_LANGUAGE: u8 = 0x36;
    pub const END_COLLECTION: u8 = 0x37;
    pub const TEXT: u8 = 0x41;
    pub const NAME: u8 = 0x42;
    pub const KEYWORD: u8 = 0x44;
    pub const URI: u8 = 0x45;
    pub const CHARSET: u8 = 0x47;
    pub const LANGUAGE: u8 = 0x48;
    pub const MIME: u8 = 0x49;
    pub const MEMBER_NAME: u8 = 0x4A;
}

/// The IPP version written in requests: 1.1, which every IPP printer understands.
const VERSION: [u8; 2] = [1, 1];
/// At most this many attributes are read from one reply.
const MAX_ATTRIBUTES: usize = 20_000;
/// Collections nested deeper than this are refused.
const MAX_DEPTH: usize = 16;

/// One attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Integer(i32),
    Boolean(bool),
    Enum(i32),
    /// Text, name, keyword, URI, charset, language, MIME type and other string types.
    Str(String),
    Bytes(Vec<u8>),
    /// A collection (its members are not kept), or an out-of-band value such as `unknown`.
    Other(u8),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Integer(n) | Value::Enum(n) => Some(*n),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    /// The group delimiter tag the attribute appeared under.
    pub group: u8,
    /// Which group it was in, counting from 0: Get-Jobs replies repeat the job group per job.
    pub section: usize,
    pub name: String,
    pub values: Vec<Value>,
}

/// A parsed IPP reply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    pub status: u16,
    pub request_id: u32,
    pub attributes: Vec<Attribute>,
}

impl Response {
    pub fn get(&self, group: u8, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.group == group && a.name == name)
    }

    fn str(&self, group: u8, name: &str) -> Option<String> {
        self.get(group, name).and_then(|a| a.values.first()).and_then(Value::as_str).map(str::to_string)
    }

    fn strs(&self, group: u8, name: &str) -> Vec<String> {
        self.get(group, name).map(|a| a.values.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    }

    fn int(&self, group: u8, name: &str) -> Option<i32> {
        self.get(group, name).and_then(|a| a.values.first()).and_then(Value::as_int)
    }

    fn ints(&self, group: u8, name: &str) -> Vec<i32> {
        self.get(group, name).map(|a| a.values.iter().filter_map(Value::as_int).collect()).unwrap_or_default()
    }

    /// Whether the status is one of the successful ones (0x0000–0x00FF).
    pub fn ok(&self) -> bool {
        self.status <= 0x00FF
    }
}

/// Builds a request.
pub struct Request {
    buf: Vec<u8>,
    group: u8,
}

impl Request {
    pub fn new(operation: u16, request_id: u32) -> Request {
        let mut buf = Vec::with_capacity(512);
        buf.extend_from_slice(&VERSION);
        buf.extend_from_slice(&operation.to_be_bytes());
        buf.extend_from_slice(&request_id.to_be_bytes());
        let mut r = Request { buf, group: 0 };
        r.group(tag::OPERATION);
        r.str(tag::CHARSET, "attributes-charset", "utf-8");
        r.str(tag::LANGUAGE, "attributes-natural-language", "en");
        r
    }

    pub fn group(&mut self, group: u8) -> &mut Self {
        if self.group != group {
            self.buf.push(group);
            self.group = group;
        }
        self
    }

    pub fn raw(&mut self, value_tag: u8, name: &str, value: &[u8]) -> &mut Self {
        // Names and values we write are short and our own; clamp anyway so a length never wraps.
        let name = name.as_bytes().get(..name.len().min(u16::MAX as usize)).unwrap_or_default();
        let value = value.get(..value.len().min(u16::MAX as usize)).unwrap_or_default();
        self.buf.push(value_tag);
        self.buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(name);
        self.buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(value);
        self
    }

    pub fn str(&mut self, value_tag: u8, name: &str, value: &str) -> &mut Self {
        self.raw(value_tag, name, value.as_bytes())
    }

    /// A further value of the attribute just written.
    pub fn also(&mut self, value_tag: u8, value: &str) -> &mut Self {
        self.raw(value_tag, "", value.as_bytes())
    }

    pub fn int(&mut self, value_tag: u8, name: &str, value: i32) -> &mut Self {
        self.raw(value_tag, name, &value.to_be_bytes())
    }

    pub fn boolean(&mut self, name: &str, value: bool) -> &mut Self {
        self.raw(tag::BOOLEAN, name, &[u8::from(value)])
    }

    pub fn bytes(&mut self, value_tag: u8, name: &str, value: &[u8]) -> &mut Self {
        self.raw(value_tag, name, value)
    }

    /// The finished message, with `document` (if any) after the end tag.
    pub fn finish(mut self, document: &[u8]) -> Vec<u8> {
        self.buf.push(tag::END);
        self.buf.extend_from_slice(document);
        self.buf
    }
}

/// Parse an IPP reply. Anything after the end-of-attributes tag is ignored.
pub fn parse(body: &[u8]) -> Result<Response, PrintError> {
    let bad = |what: &str| PrintError::Spool(format!("the printer sent a malformed IPP reply ({what})"));
    let mut r = Reader { b: body, at: 0 };
    let _version = r.take(2).ok_or_else(|| bad("too short"))?;
    let status = r.u16().ok_or_else(|| bad("too short"))?;
    let request_id = r.u32().ok_or_else(|| bad("too short"))?;
    let mut out = Response { status, request_id, attributes: Vec::new() };
    let mut group = 0u8;
    let mut section = 0usize;
    let mut depth = 0usize;
    // A reply cut short after its header keeps what was read.
    while let Some(t) = r.u8() {
        if t == tag::END {
            break;
        }
        if t < 0x10 {
            group = t;
            section = section.saturating_add(1);
            depth = 0;
            continue;
        }
        let name_len = r.u16().ok_or_else(|| bad("truncated attribute name"))? as usize;
        let name = r.take(name_len).ok_or_else(|| bad("truncated attribute name"))?;
        let value_len = r.u16().ok_or_else(|| bad("truncated value"))? as usize;
        let value = r.take(value_len).ok_or_else(|| bad("truncated value"))?;
        // Collection members are read and dropped: we only need the collection to be skipped.
        if t == tag::BEGIN_COLLECTION {
            depth = depth.checked_add(1).ok_or_else(|| bad("collection depth"))?;
            if depth > MAX_DEPTH {
                return Err(bad("collections nested too deeply"));
            }
            if depth == 1 {
                push(&mut out, (group, section), name, Value::Other(t))?;
            }
            continue;
        }
        if t == tag::END_COLLECTION {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth > 0 {
            continue;
        }
        let v = match t {
            tag::INTEGER | tag::ENUM => {
                let n = value.get(..4).and_then(|b| b.try_into().ok()).map(i32::from_be_bytes).ok_or_else(|| bad("integer"))?;
                if t == tag::ENUM { Value::Enum(n) } else { Value::Integer(n) }
            }
            tag::BOOLEAN => Value::Boolean(value.first().is_some_and(|b| *b != 0)),
            tag::TEXT_WITH_LANGUAGE | tag::NAME_WITH_LANGUAGE => {
                // a 2-byte length and language, then a 2-byte length and the text
                let mut v = Reader { b: value, at: 0 };
                let ll = v.u16().unwrap_or(0) as usize;
                let _ = v.take(ll);
                let tl = v.u16().unwrap_or(0) as usize;
                Value::Str(String::from_utf8_lossy(v.take(tl).unwrap_or_default()).into_owned())
            }
            0x40..=0x5F => Value::Str(String::from_utf8_lossy(value).into_owned()),
            tag::OCTET_STRING => Value::Bytes(value.to_vec()),
            other => Value::Other(other),
        };
        push(&mut out, (group, section), name, v)?;
    }
    Ok(out)
}

fn push(out: &mut Response, (group, section): (u8, usize), name: &[u8], v: Value) -> Result<(), PrintError> {
    if name.is_empty() {
        // An additional value of the previous attribute.
        if let Some(last) = out.attributes.last_mut()
            && last.values.len() < MAX_ATTRIBUTES
        {
            last.values.push(v);
        }
        return Ok(());
    }
    if out.attributes.len() >= MAX_ATTRIBUTES {
        return Err(PrintError::Spool("the printer's IPP reply has too many attributes".into()));
    }
    out.attributes.push(Attribute { group, section, name: String::from_utf8_lossy(name).into_owned(), values: vec![v] });
    Ok(())
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).and_then(|s| s.first().copied())
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).and_then(|s| s.try_into().ok()).map(u16::from_be_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes)
    }
}

/// A short name for an IPP status code.
pub fn status_name(code: u16) -> &'static str {
    match code {
        0x0000 => "successful-ok",
        0x0001 => "successful-ok-ignored-or-substituted-attributes",
        0x0002 => "successful-ok-conflicting-attributes",
        0x0400 => "client-error-bad-request",
        0x0401 => "client-error-forbidden",
        0x0402 => "client-error-not-authenticated",
        0x0403 => "client-error-not-authorized",
        0x0404 => "client-error-not-possible",
        0x0405 => "client-error-timeout",
        0x0406 => "client-error-not-found",
        0x0407 => "client-error-gone",
        0x0408 => "client-error-request-entity-too-large",
        0x040A => "client-error-document-format-not-supported",
        0x040B => "client-error-attributes-or-values-not-supported",
        0x040C => "client-error-uri-scheme-not-supported",
        0x040D => "client-error-charset-not-supported",
        0x040E => "client-error-conflicting-attributes",
        0x0500 => "server-error-internal-error",
        0x0501 => "server-error-operation-not-supported",
        0x0502 => "server-error-service-unavailable",
        0x0503 => "server-error-version-not-supported",
        0x0504 => "server-error-device-error",
        0x0505 => "server-error-temporary-error",
        0x0506 => "server-error-not-accepting-jobs",
        0x0507 => "server-error-busy",
        _ => "unknown-status",
    }
}

fn job_state_name(n: i32) -> String {
    match n {
        3 => "pending",
        4 => "pending-held",
        5 => "processing",
        6 => "processing-stopped",
        7 => "canceled",
        8 => "aborted",
        9 => "completed",
        _ => return format!("state-{n}"),
    }
    .to_string()
}

fn printer_state_name(n: i32) -> String {
    match n {
        3 => "idle",
        4 => "processing",
        5 => "stopped",
        _ => return format!("state-{n}"),
    }
    .to_string()
}

fn request_id() -> u32 {
    // Any non-zero id will do; vary it so replies can't be confused across requests.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1);
    (nanos & 0x7FFF_FFFF).max(1)
}

/// Send a request and parse the reply; a non-success status becomes an error.
fn call(target: &Target, message: Vec<u8>, timeout: Duration) -> Result<Response, PrintError> {
    let body = http::post(target, "application/ipp", &message, timeout)?;
    let resp = parse(&body)?;
    if !resp.ok() {
        let msg = resp.str(tag::OPERATION, "status-message");
        let detail = msg.map(|m| format!(": {}", clean_text(&m, 200))).unwrap_or_default();
        return Err(PrintError::Spool(match resp.status {
            0x040A => format!(
                "printer {} does not accept PDF{detail}; add it as a system printer (its driver converts the document) instead",
                target.authority()
            ),
            0x0402 | 0x0403 => format!("printer {} needs a sign-in to print{detail}; add it as a system printer instead", target.authority()),
            code => format!("printer {} refused the request: {} (0x{code:04x}){detail}", target.authority(), status_name(code)),
        }));
    }
    Ok(resp)
}

fn printer_uri(target: &Target) -> String {
    Target { protocol: Protocol::Ipp, ..target.clone() }.uri()
}

/// Print-Job: send the PDF with its options.
pub fn print_job(target: &Target, pdf: &[u8], job: &NetJob, timeout: Duration) -> Result<Receipt, PrintError> {
    // Ask first what the printer takes, so a printer that only reads raster formats gets a
    // clear message instead of a page of PDF source code. If it won't say, try anyway.
    let format = match printer_status(target, timeout) {
        Ok(s) if !s.takes_pdf() => {
            return Err(PrintError::Spool(format!(
                "printer {} does not accept PDF (it takes {}); add it as a system printer, whose driver converts the document",
                target.authority(),
                s.document_formats.join(", ")
            )));
        }
        Ok(s) if !s.document_formats.iter().any(|f| f.eq_ignore_ascii_case("application/pdf")) && !s.document_formats.is_empty() => {
            "application/octet-stream"
        }
        Ok(_) | Err(_) => "application/pdf",
    };
    let mut r = Request::new(op::PRINT_JOB, request_id());
    r.str(tag::URI, "printer-uri", &printer_uri(target));
    r.str(tag::NAME, "requesting-user-name", &clean_text(&job.user, 127));
    r.str(tag::NAME, "job-name", &clean_text(&job.title, 127));
    r.str(tag::MIME, "document-format", format);
    if let Some(pin) = &job.pin {
        // PWG 5100.11: the release PIN, sent in the clear (encryption "none").
        r.bytes(tag::OCTET_STRING, "job-password", pin.as_bytes());
        r.str(tag::KEYWORD, "job-password-encryption", "none");
    }
    r.group(tag::JOB);
    r.int(tag::INTEGER, "copies", job.copies.clamp(1, 999) as i32);
    if job.copies > 1 {
        let handling = if job.collate { "separate-documents-collated-copies" } else { "separate-documents-uncollated-copies" };
        r.str(tag::KEYWORD, "multiple-document-handling", handling);
    }
    if let Some(d) = job.duplex {
        let sides = match d {
            Duplex::Off => "one-sided",
            Duplex::LongEdge => "two-sided-long-edge",
            Duplex::ShortEdge => "two-sided-short-edge",
        };
        r.str(tag::KEYWORD, "sides", sides);
    }
    match job.color {
        ColorMode::Auto => {}
        ColorMode::Color => {
            r.str(tag::KEYWORD, "print-color-mode", "color");
        }
        ColorMode::Monochrome => {
            r.str(tag::KEYWORD, "print-color-mode", "monochrome");
        }
    }
    if job.held() {
        r.str(tag::KEYWORD, "job-hold-until", "indefinite");
    }
    if let Some(account) = &job.account {
        let account = clean_text(account, 127);
        if !account.is_empty() {
            r.str(tag::NAME, "job-account-id", &account);
        }
    }
    if let Some((first, rest)) = job.finishings.split_first() {
        r.int(tag::ENUM, "finishings", first.ipp());
        for f in rest {
            r.raw(tag::ENUM, "", &f.ipp().to_be_bytes());
        }
    }
    // Sheets are already laid out at their physical size: the printer must not scale them.
    r.str(tag::KEYWORD, "print-scaling", "none");
    let resp = call(target, r.finish(pdf), timeout)?;
    let ignored = resp.attributes.iter().filter(|a| a.group == tag::UNSUPPORTED).map(|a| a.name.clone()).collect();
    Ok(Receipt {
        protocol: Protocol::Ipp,
        job_id: resp.int(tag::JOB, "job-id"),
        job_uri: resp.str(tag::JOB, "job-uri"),
        state: resp.int(tag::JOB, "job-state").map(job_state_name),
        ignored,
        bytes: pdf.len(),
    })
}

/// Get-Printer-Attributes.
pub fn printer_status(target: &Target, timeout: Duration) -> Result<PrinterStatus, PrintError> {
    let mut r = Request::new(op::GET_PRINTER_ATTRIBUTES, request_id());
    r.str(tag::URI, "printer-uri", &printer_uri(target));
    r.str(tag::NAME, "requesting-user-name", &super::default_user());
    r.str(tag::KEYWORD, "requested-attributes", "printer-description");
    for a in ["printer-state", "printer-state-reasons", "printer-state-message", "queued-job-count", "marker-names", "marker-levels"] {
        r.also(tag::KEYWORD, a);
    }
    let resp = call(target, r.finish(&[]), timeout)?;
    Ok(status_from(&resp))
}

pub fn status_from(resp: &Response) -> PrinterStatus {
    let p = tag::PRINTER;
    let names = resp.strs(p, "marker-names");
    let levels = resp.ints(p, "marker-levels");
    PrinterStatus {
        name: resp.str(p, "printer-name"),
        make_and_model: resp.str(p, "printer-make-and-model"),
        location: resp.str(p, "printer-location").filter(|s| !s.is_empty()),
        state: resp.int(p, "printer-state").map(printer_state_name),
        state_reasons: resp.strs(p, "printer-state-reasons").into_iter().filter(|r| r != "none").collect(),
        state_message: resp.str(p, "printer-state-message").filter(|s| !s.is_empty()),
        accepting_jobs: resp.get(p, "printer-is-accepting-jobs").and_then(|a| a.values.first()).and_then(|v| match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }),
        queued_jobs: resp.int(p, "queued-job-count"),
        document_formats: resp.strs(p, "document-format-supported"),
        sides: resp.strs(p, "sides-supported"),
        color: resp.get(p, "color-supported").and_then(|a| a.values.first()).and_then(|v| match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        }),
        supplies: names.into_iter().zip(levels).collect(),
    }
}

const JOB_ATTRIBUTES: [&str; 6] = ["job-id", "job-name", "job-originating-user-name", "job-state", "job-state-reasons", "job-impressions-completed"];

fn job_from(attrs: &[&Attribute], fallback_id: i32) -> JobStatus {
    let find = |n: &str| attrs.iter().find(|a| a.name == n).and_then(|a| a.values.first());
    JobStatus {
        job_id: find("job-id").and_then(Value::as_int).unwrap_or(fallback_id),
        name: find("job-name").and_then(Value::as_str).map(str::to_string),
        user: find("job-originating-user-name").and_then(Value::as_str).map(str::to_string),
        state: find("job-state").and_then(Value::as_int).map(job_state_name),
        state_reasons: attrs
            .iter()
            .find(|a| a.name == "job-state-reasons")
            .map(|a| a.values.iter().filter_map(Value::as_str).filter(|r| *r != "none").map(str::to_string).collect())
            .unwrap_or_default(),
        sheets_completed: find("job-impressions-completed").and_then(Value::as_int),
    }
}

pub fn job_status(target: &Target, job_id: i32, timeout: Duration) -> Result<JobStatus, PrintError> {
    let mut r = Request::new(op::GET_JOB_ATTRIBUTES, request_id());
    r.str(tag::URI, "printer-uri", &printer_uri(target));
    r.int(tag::INTEGER, "job-id", job_id);
    r.str(tag::NAME, "requesting-user-name", &super::default_user());
    let (first, rest) = JOB_ATTRIBUTES.split_first().unwrap_or((&"job-id", &[]));
    r.str(tag::KEYWORD, "requested-attributes", first);
    for a in rest {
        r.also(tag::KEYWORD, a);
    }
    let resp = call(target, r.finish(&[]), timeout)?;
    let attrs: Vec<&Attribute> = resp.attributes.iter().filter(|a| a.group == tag::JOB).collect();
    Ok(job_from(&attrs, job_id))
}

pub fn jobs(target: &Target, completed: bool, mine_only: Option<&str>, timeout: Duration) -> Result<Vec<JobStatus>, PrintError> {
    let mut r = Request::new(op::GET_JOBS, request_id());
    r.str(tag::URI, "printer-uri", &printer_uri(target));
    let user = mine_only.map(|u| clean_text(u, 127)).unwrap_or_else(super::default_user);
    r.str(tag::NAME, "requesting-user-name", &user);
    r.int(tag::INTEGER, "limit", 500);
    r.str(tag::KEYWORD, "which-jobs", if completed { "completed" } else { "not-completed" });
    if mine_only.is_some() {
        r.boolean("my-jobs", true);
    }
    let (first, rest) = JOB_ATTRIBUTES.split_first().unwrap_or((&"job-id", &[]));
    r.str(tag::KEYWORD, "requested-attributes", first);
    for a in rest {
        r.also(tag::KEYWORD, a);
    }
    let resp = call(target, r.finish(&[]), timeout)?;
    // Each job is its own job-attributes group.
    let mut out = Vec::new();
    let mut current: Vec<&Attribute> = Vec::new();
    for a in resp.attributes.iter().filter(|a| a.group == tag::JOB) {
        if current.first().is_some_and(|c| c.section != a.section) {
            out.push(job_from(&current, 0));
            current = Vec::new();
        }
        current.push(a);
    }
    if !current.is_empty() {
        out.push(job_from(&current, 0));
    }
    Ok(out)
}

/// Cancel-Job or Release-Job.
pub fn simple_job_op(target: &Target, operation: u16, job_id: i32, user: &str, timeout: Duration) -> Result<(), PrintError> {
    let mut r = Request::new(operation, request_id());
    r.str(tag::URI, "printer-uri", &printer_uri(target));
    r.int(tag::INTEGER, "job-id", job_id);
    r.str(tag::NAME, "requesting-user-name", &clean_text(user, 127));
    call(target, r.finish(&[]), timeout).map(|_| ())
}
