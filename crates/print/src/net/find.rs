//! Finding printers that multicast DNS alone misses.
//!
//! Office printers are often invisible to Bonjour: the vendor ships it switched off (many Canon
//! imageRUNNER, Ricoh and Kyocera models answer WS-Discovery instead), the network blocks
//! multicast, the printers sit in another VLAN, or a VPN takes the default route so the query
//! never reaches the office network at all. [`search`] therefore combines:
//!
//! - multicast DNS sent through **every** IPv4 interface ([`super::mdns`]);
//! - a WS-Discovery probe (the protocol Windows uses for "WSD" printers), every interface;
//! - a TCP check of IPP (631), raw (9100) and LPD (515) on each address of the local subnets
//!   (at most [`MAX_SCAN_HOSTS`] per subnet) and of ranges the user names (`10.20.30.0/24`,
//!   `10.20.30.10-80`, `10.20.30.10-10.20.30.80` or one address), which also reaches printers in
//!   other subnets that multicast never crosses.
//!
//! An address that answers IPP is asked for its name, model, location and the formats it
//! takes, trying the resource paths printers commonly use. Every reply is untrusted: WS-Discovery
//! datagrams are only searched for a bounded keyword, and IPP replies go through the bounded
//! parser in [`super::ipp`].

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::mdns::Found;
use super::{Protocol, Target};
use crate::PrintError;

/// The most addresses checked in one subnet or range.
pub const MAX_SCAN_HOSTS: usize = 1024;
/// The most addresses checked in one search, over every subnet and range.
pub const MAX_TOTAL_HOSTS: usize = 4096;
/// Connections tried at once.
const WORKERS: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(350);
const IPP_TIMEOUT: Duration = Duration::from_secs(3);
/// IPP resource paths, most common first (`/ipp/lp` is Canon's, `/ipp/print` IPP Everywhere's).
const IPP_PATHS: [&str; 5] = ["/ipp/print", "/ipp", "/ipp/lp", "/printer", "/"];
const WSD: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 3702));

/// What to search.
#[derive(Clone, Debug)]
pub struct SearchOptions {
    /// How long multicast and WS-Discovery replies are awaited.
    pub wait: Duration,
    /// Check every address of this computer's own subnets (up to [`MAX_SCAN_HOSTS`] each).
    pub scan_local: bool,
    /// More addresses to check: `10.0.0.0/24`, `10.0.0.10-80`, `10.0.0.10-10.0.0.80`, `10.0.0.7`.
    pub ranges: Vec<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions { wait: Duration::from_secs(2), scan_local: true, ranges: Vec::new() }
    }
}

/// This computer's IPv4 interfaces (address and netmask), loopback and link-local left out.
pub fn interfaces() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut out: Vec<(Ipv4Addr, Ipv4Addr)> = Vec::new();
        for i in if_addrs::get_if_addrs().unwrap_or_default() {
            if let if_addrs::IfAddr::V4(v4) = &i.addr
                && !v4.ip.is_loopback()
                && !v4.ip.is_link_local()
                && !v4.ip.is_unspecified()
                && !out.iter().any(|(ip, _)| *ip == v4.ip)
            {
                out.push((v4.ip, v4.netmask));
            }
        }
        out.truncate(32);
        out
    }
    #[cfg(target_arch = "wasm32")]
    {
        Vec::new()
    }
}

/// Every address of an interface's subnet except its network, broadcast and own addresses, or
/// nothing when the subnet is larger than [`MAX_SCAN_HOSTS`] or a point-to-point link (a VPN).
pub fn subnet_hosts(ip: Ipv4Addr, mask: Ipv4Addr) -> Vec<Ipv4Addr> {
    let m = u32::from(mask);
    let prefix = m.leading_ones();
    // Not a contiguous mask, or /31 and /32 (point to point): nothing to scan.
    if m.count_ones() != prefix || prefix >= 31 || prefix == 0 {
        return Vec::new();
    }
    let size = 1u64 << (32 - prefix);
    if size.saturating_sub(2) > MAX_SCAN_HOSTS as u64 {
        return Vec::new();
    }
    let net = u32::from(ip) & m;
    let broadcast = net | !m;
    (net.saturating_add(1)..broadcast).map(Ipv4Addr::from).filter(|h| *h != ip).collect()
}

/// Parse a range the user typed: `10.0.0.0/24`, `10.0.0.10-80`, `10.0.0.10-10.0.0.80` or one
/// address. Ranges above [`MAX_SCAN_HOSTS`] addresses are refused.
pub fn parse_range(text: &str) -> Result<Vec<Ipv4Addr>, PrintError> {
    let t = text.trim();
    let bad = || PrintError::Invalid(format!("not an address range: {t:?} (use 10.0.0.0/24, 10.0.0.10-80 or 10.0.0.7)"));
    let too_big = || PrintError::Invalid(format!("{t} has more than {MAX_SCAN_HOSTS} addresses; name a smaller range"));
    if t.is_empty() || t.len() > 64 {
        return Err(bad());
    }
    let (first, last) = if let Some((ip, bits)) = t.split_once('/') {
        let ip: Ipv4Addr = ip.trim().parse().map_err(|_| bad())?;
        let bits: u32 = bits.trim().parse().map_err(|_| bad())?;
        if !(16..=32).contains(&bits) {
            return Err(if bits < 16 { too_big() } else { bad() });
        }
        if bits >= 31 {
            (u32::from(ip), u32::from(ip))
        } else {
            let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
            let net = u32::from(ip) & mask;
            (net.saturating_add(1), (net | !mask).saturating_sub(1))
        }
    } else if let Some((a, b)) = t.split_once('-') {
        let a: Ipv4Addr = a.trim().parse().map_err(|_| bad())?;
        let b = b.trim();
        let b: Ipv4Addr = match b.parse::<u8>() {
            Ok(last_octet) => {
                let o = a.octets();
                Ipv4Addr::new(o[0], o[1], o[2], last_octet)
            }
            Err(_) => b.parse().map_err(|_| bad())?,
        };
        (u32::from(a), u32::from(b))
    } else {
        let ip: Ipv4Addr = t.parse().map_err(|_| bad())?;
        (u32::from(ip), u32::from(ip))
    };
    if last < first {
        return Err(bad());
    }
    if u64::from(last - first) + 1 > MAX_SCAN_HOSTS as u64 {
        return Err(too_big());
    }
    Ok((first..=last).map(Ipv4Addr::from).collect())
}

/// Search the network: multicast DNS and WS-Discovery on every interface, then (when asked) a
/// check of the local subnets and the given ranges. Printers come back once each, by address.
pub fn search(opts: &SearchOptions) -> Result<Vec<Found>, PrintError> {
    let mut hosts: Vec<Ipv4Addr> = Vec::new();
    for r in &opts.ranges {
        hosts.extend(parse_range(r)?);
    }
    let ifaces = interfaces();
    let wait = opts.wait.clamp(Duration::from_millis(200), Duration::from_secs(30));
    // Multicast DNS and WS-Discovery at the same time.
    let (mut found, wsd_hosts) = std::thread::scope(|s| {
        let wsd = s.spawn(|| ws_discovery(&ifaces, wait));
        let mdns = super::mdns::discover_on(&ifaces, wait);
        (mdns, wsd.join().unwrap_or_default())
    });
    let found_list = found.as_mut().map(std::mem::take).unwrap_or_default();
    let mdns_error = found.err();
    hosts.extend(wsd_hosts);
    if opts.scan_local {
        for (ip, mask) in &ifaces {
            hosts.extend(subnet_hosts(*ip, *mask));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let known: std::collections::BTreeSet<IpAddr> = found_list.iter().filter_map(|f| f.address).collect();
    hosts.retain(|h| seen.insert(*h) && !known.contains(&IpAddr::V4(*h)));
    hosts.truncate(MAX_TOTAL_HOSTS);
    let mut all = found_list;
    all.extend(probe_hosts(&hosts));
    if all.is_empty()
        && let Some(e) = mdns_error
        && ifaces.is_empty()
    {
        return Err(e);
    }
    Ok(merge(all))
}

/// One printer per address and protocol, IPP first.
fn merge(found: Vec<Found>) -> Vec<Found> {
    let rank = |p: Protocol| match p {
        Protocol::Ipp => 0,
        Protocol::Ipps => 1,
        Protocol::Raw => 2,
        Protocol::Lpd => 3,
    };
    let mut by_key: BTreeMap<(String, u8), Found> = BTreeMap::new();
    for f in found {
        let host = f.address.map(|a| a.to_string()).unwrap_or_else(|| f.host.to_ascii_lowercase());
        by_key.entry((host, rank(f.protocol))).or_insert(f);
    }
    let mut out: Vec<Found> = by_key.into_values().collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(rank(a.protocol).cmp(&rank(b.protocol))));
    out
}

/// Check many addresses at once, [`WORKERS`] connections at a time.
pub fn probe_hosts(hosts: &[Ipv4Addr]) -> Vec<Found> {
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(hosts.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(h) = hosts.get(i) else { break };
                    let found = probe_host(*h);
                    if !found.is_empty()
                        && let Ok(mut o) = out.lock()
                    {
                        o.extend(found);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_default()
}

fn open(ip: Ipv4Addr, port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::V4(SocketAddrV4::new(ip, port)), CONNECT_TIMEOUT).is_ok()
}

/// What one address offers: an IPP printer (with its details) and, when it listens there, its
/// raw and LPD ports. Nothing when no printer port answers.
pub fn probe_host(ip: Ipv4Addr) -> Vec<Found> {
    probe_host_on(ip, [631, 9100, 515])
}

/// [`probe_host`] with the IPP, raw and LPD ports given (tests use loopback ports).
pub(crate) fn probe_host_on(ip: Ipv4Addr, [ipp_port, raw_port, lpd_port]: [u16; 3]) -> Vec<Found> {
    let mut out = Vec::new();
    let ipp = open(ip, ipp_port).then(|| ask_ipp(ip, ipp_port)).flatten();
    let raw = open(ip, raw_port);
    let lpd = !raw && ipp.is_none() && open(ip, lpd_port);
    let name = ipp.as_ref().map(|f| f.name.clone());
    let model = ipp.as_ref().and_then(|f| f.make_and_model.clone());
    let location = ipp.as_ref().and_then(|f| f.location.clone());
    let pdf = ipp.as_ref().and_then(|f| f.pdf);
    let label = |what: &str| match &name {
        Some(n) => format!("{n} ({what})"),
        None => format!("{ip} ({what})"),
    };
    let plain = |protocol: Protocol, what: &str, path: &str| Found {
        name: label(what),
        protocol,
        host: ip.to_string(),
        address: Some(IpAddr::V4(ip)),
        port: if protocol == Protocol::Raw { raw_port } else { lpd_port },
        path: path.into(),
        make_and_model: model.clone(),
        location: location.clone(),
        pdf,
        color: None,
        duplex: None,
    };
    if raw {
        out.push(plain(Protocol::Raw, "port 9100", ""));
    }
    if lpd {
        out.push(plain(Protocol::Lpd, "LPD", "lp"));
    }
    if let Some(f) = ipp {
        out.insert(0, f);
    }
    out
}

/// Ask an address that listens on 631 who it is, trying the usual resource paths.
fn ask_ipp(ip: Ipv4Addr, port: u16) -> Option<Found> {
    for path in IPP_PATHS {
        let target = Target { protocol: Protocol::Ipp, host: ip.to_string(), port, path: path.into() };
        let Ok(s) = super::ipp::printer_status(&target, IPP_TIMEOUT) else { continue };
        let pdf = if s.document_formats.is_empty() { None } else { Some(s.takes_pdf()) };
        let name = s.name.clone().filter(|n| !n.trim().is_empty()).or_else(|| s.make_and_model.clone()).unwrap_or_else(|| ip.to_string());
        return Some(Found {
            name: super::clean_text(&name, 120),
            protocol: Protocol::Ipp,
            host: ip.to_string(),
            address: Some(IpAddr::V4(ip)),
            port,
            path: path.into(),
            make_and_model: s.make_and_model.map(|m| super::clean_text(&m, 120)),
            location: s.location.map(|l| super::clean_text(&l, 120)).filter(|l| !l.is_empty()),
            pdf,
            color: s.color,
            duplex: if s.sides.is_empty() { None } else { Some(s.sides.iter().any(|x| x.starts_with("two-sided"))) },
        });
    }
    None
}

/// A WS-Discovery Probe (SOAP over UDP, the protocol behind Windows' "WSD" printers).
fn probe_message() -> Vec<u8> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
    let id = format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (nanos >> 32) as u32,
        (nanos >> 16) as u16,
        (nanos & 0xFFF) as u16,
        ((nanos >> 12) & 0xFFF) as u16,
        (nanos & 0xFFFF_FFFF_FFFF) as u64
    );
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:wsa=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" \
xmlns:wsd=\"http://schemas.xmlsoap.org/ws/2005/04/discovery\" \
xmlns:wprt=\"http://schemas.microsoft.com/windows/2006/08/wdp/print\">\
<soap:Header><wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To>\
<wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action>\
<wsa:MessageID>urn:uuid:{id}</wsa:MessageID></soap:Header>\
<soap:Body><wsd:Probe><wsd:Types>wprt:PrintDeviceType</wsd:Types></wsd:Probe></soap:Body></soap:Envelope>"
    )
    .into_bytes()
}

/// Addresses of the devices that answer a WS-Discovery probe for printers, through every
/// interface. A reply only counts when it is a ProbeMatch that mentions printing; its sender's
/// address is used, never an address written inside the reply.
pub fn ws_discovery(ifaces: &[(Ipv4Addr, Ipv4Addr)], wait: Duration) -> Vec<Ipv4Addr> {
    let mut sockets = Vec::new();
    let mut locals: Vec<Ipv4Addr> = ifaces.iter().map(|(ip, _)| *ip).collect();
    if locals.is_empty() {
        locals.push(Ipv4Addr::UNSPECIFIED);
    }
    let message = probe_message();
    for ip in locals {
        let Ok(sock) = UdpSocket::bind((ip, 0)) else { continue };
        let _ = sock.set_multicast_ttl_v4(4);
        if sock.send_to(&message, WSD).is_ok() {
            sockets.push(sock);
        }
    }
    let found = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for sock in &sockets {
            s.spawn(|| {
                let deadline = Instant::now() + wait;
                let mut buf = vec![0u8; 16 * 1024];
                let mut mine = Vec::new();
                while mine.len() < 256 {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() || sock.set_read_timeout(Some(left.max(Duration::from_millis(1)))).is_err() {
                        break;
                    }
                    match sock.recv_from(&mut buf) {
                        Ok((n, SocketAddr::V4(from))) => {
                            if is_printer_match(buf.get(..n).unwrap_or_default()) {
                                mine.push(*from.ip());
                            }
                        }
                        Ok(_) => {}
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
                        Err(_) => {}
                    }
                }
                if let Ok(mut f) = found.lock() {
                    f.extend(mine);
                }
            });
        }
    });
    let mut out = found.into_inner().unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// Whether this computer has a point-to-point interface (a /32 address), the shape VPN tunnels
/// take. When a search finds nothing, a VPN that routes the local network through its tunnel is
/// the usual reason.
pub fn vpn_suspected() -> bool {
    interfaces().iter().any(|(_, mask)| u32::from(*mask) == u32::MAX)
}

/// Whether a WS-Discovery datagram is a ProbeMatch from a print device.
pub fn is_printer_match(datagram: &[u8]) -> bool {
    let text = String::from_utf8_lossy(datagram.get(..datagram.len().min(64 * 1024)).unwrap_or_default()).to_ascii_lowercase();
    text.contains("probematch") && text.contains("print")
}
