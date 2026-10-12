//! Finding printers on the local network with multicast DNS service discovery (RFC 6762/6763),
//! the way Bonjour / AirPrint / Mopria do it.
//!
//! We send "legacy unicast" queries (from an ordinary port, so nothing needs port 5353) for the
//! printer service types; responders answer us directly. Each answer names a printer instance,
//! where it listens (SRV), what it is (TXT: model, location, formats, colour, duplex) and its
//! addresses (A/AAAA). Packets come from anyone on the network, so the parser bounds every
//! length, caps compression-pointer chains and keeps at most a few hundred records.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use super::{Protocol, Target};
use crate::PrintError;

/// The service types printers advertise, with the protocol each one is reached with.
pub const SERVICES: [(&str, Protocol); 4] = [
    ("_ipp._tcp.local", Protocol::Ipp),
    ("_ipps._tcp.local", Protocol::Ipps),
    ("_pdl-datastream._tcp.local", Protocol::Raw),
    ("_printer._tcp.local", Protocol::Lpd),
];

const MDNS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(224, 0, 0, 251)), 5353);
const MAX_RECORDS: usize = 2_000;
const MAX_FOUND: usize = 256;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;

/// One DNS resource record we care about.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Ptr { name: String, target: String },
    Srv { name: String, port: u16, target: String },
    Txt { name: String, entries: Vec<(String, String)> },
    Addr { name: String, addr: IpAddr },
}

/// A printer found on the network.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// Its advertised name, e.g. "Office Laser (2nd floor)".
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub address: Option<IpAddr>,
    pub port: u16,
    /// IPP resource path (`rp`) or LPD queue.
    pub path: String,
    pub make_and_model: Option<String>,
    pub location: Option<String>,
    /// Whether it says it takes PDF (TXT `pdl`); `None` when it doesn't say.
    pub pdf: Option<bool>,
    pub color: Option<bool>,
    pub duplex: Option<bool>,
}

impl Found {
    /// The printer's address to print to. The numeric address is preferred, because `.local`
    /// names don't resolve everywhere.
    pub fn target(&self) -> Target {
        let host = self.address.map(|a| a.to_string()).unwrap_or_else(|| self.host.trim_end_matches('.').to_string());
        let path = match self.protocol {
            Protocol::Ipp | Protocol::Ipps => {
                let p = self.path.trim_start_matches('/');
                format!("/{}", if p.is_empty() { "ipp/print" } else { p })
            }
            Protocol::Lpd => {
                let q = self.path.trim_start_matches('/');
                if q.is_empty() { "lp".into() } else { q.to_string() }
            }
            Protocol::Raw => String::new(),
        };
        Target { protocol: self.protocol, host, port: self.port, path }
    }

    pub fn uri(&self) -> String {
        self.target().uri()
    }
}

/// A DNS query for `names` (PTR) asking for unicast replies.
pub fn query(id: u16, questions: &[(&str, u16)]) -> Vec<u8> {
    let mut q = Vec::with_capacity(256);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0u16.to_be_bytes()); // flags: standard query
    q.extend_from_slice(&(questions.len().min(u16::MAX as usize) as u16).to_be_bytes());
    q.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for (name, qtype) in questions {
        for label in name.trim_end_matches('.').split('.') {
            let label = label.as_bytes();
            let label = label.get(..label.len().min(63)).unwrap_or_default();
            q.push(label.len() as u8);
            q.extend_from_slice(label);
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&0x8001u16.to_be_bytes()); // class IN, unicast response wanted
    }
    q
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
    fn u16(&mut self) -> Option<u16> {
        self.take(2).and_then(|s| s.try_into().ok()).map(u16::from_be_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).and_then(|s| s.try_into().ok()).map(u32::from_be_bytes)
    }
}

/// Read a (possibly compressed) domain name starting at `at` in `msg`; returns the name and the
/// position just after it in the uncompressed stream.
fn read_name(msg: &[u8], at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = at;
    let mut end = None;
    let mut jumps = 0;
    let mut total = 0usize;
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            if end.is_none() {
                end = Some(pos.checked_add(1)?);
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            let lo = *msg.get(pos.checked_add(1)?)? as usize;
            let ptr = ((len & 0x3F) << 8) | lo;
            if end.is_none() {
                end = Some(pos.checked_add(2)?);
            }
            jumps += 1;
            if jumps > 32 || ptr >= msg.len() {
                return None;
            }
            pos = ptr;
            continue;
        }
        if len & 0xC0 != 0 {
            return None; // reserved label types
        }
        let start = pos.checked_add(1)?;
        let label = msg.get(start..start.checked_add(len)?)?;
        total = total.checked_add(len + 1)?;
        if total > 255 {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).into_owned());
        pos = start + len;
    }
    Some((labels.join("."), end?))
}

/// Parse one DNS message into the records we use. Malformed messages yield what was readable
/// before the fault.
pub fn parse(msg: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut r = Reader { b: msg, at: 0 };
    let Some(header) = r.take(12) else { return out };
    let count = |i: usize| header.get(i..i + 2).and_then(|s| s.try_into().ok()).map(u16::from_be_bytes).unwrap_or(0) as usize;
    let (qd, an, ns, ar) = (count(4), count(6), count(8), count(10));
    // Skip the questions.
    for _ in 0..qd.min(64) {
        let Some((_, next)) = read_name(msg, r.at) else { return out };
        r.at = next;
        if r.take(4).is_none() {
            return out;
        }
    }
    for _ in 0..(an + ns + ar).min(MAX_RECORDS) {
        let Some((name, next)) = read_name(msg, r.at) else { return out };
        r.at = next;
        let (Some(rtype), Some(_class), Some(_ttl), Some(len)) = (r.u16(), r.u16(), r.u32(), r.u16()) else { return out };
        let rdata_at = r.at;
        let Some(rdata) = r.take(len as usize) else { return out };
        let rec = match rtype {
            TYPE_PTR => read_name(msg, rdata_at).map(|(target, _)| Record::Ptr { name, target }),
            TYPE_SRV => {
                let port = rdata.get(4..6).and_then(|s| s.try_into().ok()).map(u16::from_be_bytes);
                let target = rdata_at.checked_add(6).and_then(|at| read_name(msg, at)).map(|(t, _)| t);
                match (port, target) {
                    (Some(port), Some(target)) => Some(Record::Srv { name, port, target }),
                    _ => None,
                }
            }
            TYPE_TXT => {
                let mut entries = Vec::new();
                let mut t = Reader { b: rdata, at: 0 };
                while let Some(len) = t.take(1).and_then(|b| b.first().copied()) {
                    let Some(s) = t.take(len as usize) else { break };
                    let s = String::from_utf8_lossy(s);
                    let (k, v) = s.split_once('=').unwrap_or((&s, ""));
                    if entries.len() < 64 {
                        entries.push((k.to_ascii_lowercase(), v.to_string()));
                    }
                }
                Some(Record::Txt { name, entries })
            }
            TYPE_A => <[u8; 4]>::try_from(rdata).ok().map(|b| Record::Addr { name, addr: IpAddr::V4(Ipv4Addr::from(b)) }),
            TYPE_AAAA => <[u8; 16]>::try_from(rdata).ok().map(|b| Record::Addr { name, addr: IpAddr::V6(Ipv6Addr::from(b)) }),
            _ => None,
        };
        if let Some(rec) = rec {
            out.push(rec);
        }
    }
    out
}

fn lower(s: &str) -> String {
    s.trim_end_matches('.').to_ascii_lowercase()
}

/// Turn the records gathered from every reply into printers, one per instance and protocol.
pub fn assemble(records: &[Record]) -> Vec<Found> {
    let mut srv: BTreeMap<String, (u16, String)> = BTreeMap::new();
    let mut txt: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut addrs: BTreeMap<String, Vec<IpAddr>> = BTreeMap::new();
    let mut instances: Vec<(String, Protocol)> = Vec::new();
    for r in records {
        match r {
            Record::Ptr { name, target } => {
                if let Some((_, p)) = SERVICES.iter().find(|(s, _)| lower(name) == *s)
                    && !instances.iter().any(|(i, q)| lower(i) == lower(target) && q == p)
                {
                    instances.push((target.clone(), *p));
                }
            }
            Record::Srv { name, port, target } => {
                srv.insert(lower(name), (*port, target.clone()));
            }
            Record::Txt { name, entries } => {
                txt.insert(lower(name), entries.clone());
            }
            Record::Addr { name, addr } => {
                let list = addrs.entry(lower(name)).or_default();
                if !list.contains(addr) && list.len() < 8 {
                    list.push(*addr);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (instance, protocol) in instances {
        let Some((port, host)) = srv.get(&lower(&instance)) else { continue };
        let entries = txt.get(&lower(&instance)).cloned().unwrap_or_default();
        let get = |k: &str| entries.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
        let flag = |k: &str| get(k).map(|v| v.eq_ignore_ascii_case("t") || v.eq_ignore_ascii_case("true"));
        let service = SERVICES.iter().find(|(_, p)| *p == protocol).map(|(s, _)| *s).unwrap_or("");
        let suffix = format!(".{service}");
        let name = instance.strip_suffix(&suffix).or_else(|| instance.strip_suffix(&format!("{suffix}."))).unwrap_or(&instance);
        let name: String = name.replace("\\032", " ").chars().filter(|c| !c.is_control()).take(120).collect();
        // Prefer an IPv4 address: link-local IPv6 needs a zone that a URI can't carry everywhere.
        let list = addrs.get(&lower(host)).cloned().unwrap_or_default();
        let address = list.iter().find(|a| a.is_ipv4()).or_else(|| list.first()).copied();
        out.push(Found {
            name,
            protocol,
            host: host.trim_end_matches('.').to_string(),
            address,
            port: *port,
            path: get("rp").unwrap_or_default(),
            make_and_model: get("ty").or_else(|| get("product").map(|p| p.trim_matches(['(', ')']).to_string())),
            location: get("note"),
            pdf: get("pdl").map(|p| p.split(',').any(|f| f.trim().eq_ignore_ascii_case("application/pdf"))),
            color: flag("color"),
            duplex: flag("duplex"),
        });
        if out.len() >= MAX_FOUND {
            break;
        }
    }
    // One printer usually advertises several services: list IPP first, then the others.
    let rank = |p: Protocol| match p {
        Protocol::Ipp => 0,
        Protocol::Ipps => 1,
        Protocol::Raw => 2,
        Protocol::Lpd => 3,
    };
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(rank(a.protocol).cmp(&rank(b.protocol))));
    out
}

/// Look for printers for about two seconds.
pub fn discover() -> Result<Vec<Found>, PrintError> {
    discover_with(Duration::from_secs(2))
}

/// Look for printers for `wait`, asking again once for the details of printers whose first
/// replies left something out.
pub fn discover_with(wait: Duration) -> Result<Vec<Found>, PrintError> {
    let wait = wait.clamp(Duration::from_millis(200), Duration::from_secs(30));
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| PrintError::Spool(format!("cannot search the network for printers: {e}")))?;
    let _ = sock.set_multicast_ttl_v4(255);
    let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1) & 0xFFFF) as u16;
    let questions: Vec<(&str, u16)> = SERVICES.iter().map(|(s, _)| (*s, TYPE_PTR)).collect();
    sock.send_to(&query(id, &questions), MDNS).map_err(|e| PrintError::Spool(format!("cannot search the network for printers: {e}")))?;
    let mut records = Vec::new();
    let half = wait / 2;
    collect(&sock, half, &mut records);

    // Ask for whatever the first replies left out: an instance's SRV/TXT, a host's address.
    let partial = missing(&records);
    if !partial.is_empty() {
        let qs: Vec<(&str, u16)> = partial.iter().take(32).map(|n| (n.as_str(), TYPE_ANY)).collect();
        let _ = sock.send_to(&query(id.wrapping_add(1), &qs), MDNS);
    }
    collect(&sock, wait.saturating_sub(half), &mut records);
    Ok(assemble(&records))
}

fn collect(sock: &UdpSocket, wait: Duration, records: &mut Vec<Record>) {
    let deadline = Instant::now() + wait;
    let mut buf = vec![0u8; 9000];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || records.len() >= MAX_RECORDS {
            return;
        }
        if sock.set_read_timeout(Some(left.max(Duration::from_millis(1)))).is_err() {
            return;
        }
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                let room = MAX_RECORDS.saturating_sub(records.len());
                records.extend(parse(buf.get(..n).unwrap_or_default()).into_iter().take(room));
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return,
            // A stray ICMP error or an interrupted call: keep listening.
            Err(_) => continue,
        }
    }
}

/// Names whose details are still missing after the first replies.
fn missing(records: &[Record]) -> Vec<String> {
    let mut out = Vec::new();
    for r in records {
        if let Record::Ptr { name, target } = r
            && SERVICES.iter().any(|(s, _)| lower(name) == *s)
        {
            let srv = records.iter().find_map(|x| match x {
                Record::Srv { name, target: host, .. } if lower(name) == lower(target) => Some(host.clone()),
                _ => None,
            });
            match srv {
                None => out.push(target.clone()),
                Some(host) => {
                    let has_addr = records.iter().any(|x| matches!(x, Record::Addr { name, .. } if lower(name) == lower(&host)));
                    if !has_addr {
                        out.push(host);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}
