//! The Line Printer Daemon protocol (RFC 1179): send a job to a named queue, or ask a queue
//! for its state.
//!
//! The job's options go in a PJL header around the document (see [`super::pjl`]); copies are
//! also asked for the LPD way, with one print line per copy in the control file.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::{NetJob, PrinterStatus, Protocol, Receipt, Target, clean_text, io, pjl};
use crate::PrintError;

/// Queue state replies longer than this are cut.
const MAX_STATUS: u64 = 64 * 1024;

fn ack(s: &mut TcpStream, target: &Target, step: &str) -> Result<(), PrintError> {
    let mut b = [0u8; 1];
    s.read_exact(&mut b).map_err(io(target))?;
    if b == [0] {
        Ok(())
    } else {
        Err(PrintError::Spool(format!("printer {} refused the job ({step}); check the queue name {:?}", target.authority(), target.path)))
    }
}

fn local_host() -> String {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(31).collect();
    if name.is_empty() { "prckopis".into() } else { name }
}

/// The LPD control file for one job (RFC 1179 §7).
pub fn control_file(job: &NetJob, host: &str, data_name: &str) -> String {
    let user = clean_text(&job.user, 31).replace(' ', "_");
    let user = if user.is_empty() { "prckopis".to_string() } else { user };
    let title = clean_text(&job.title, 99);
    let mut c = format!("H{host}\nP{user}\nJ{title}\nN{title}\n");
    // 'l' prints the file as it is, control characters included (binary-safe).
    for _ in 0..job.copies.clamp(1, 999) {
        c.push_str(&format!("l{data_name}\n"));
    }
    c.push_str(&format!("U{data_name}\n"));
    c
}

pub fn print(target: &Target, pdf: &[u8], job: &NetJob, timeout: Duration) -> Result<Receipt, PrintError> {
    if job.pjl {
        // PJL carries the copies itself: one print line, so the printer doesn't multiply them.
        let once = NetJob { copies: 1, ..job.clone() };
        send(target, &pjl::wrap(pdf, job), &once, timeout)
    } else {
        send(target, pdf, job, timeout)
    }
}

fn send(target: &Target, data: &[u8], job: &NetJob, timeout: Duration) -> Result<Receipt, PrintError> {
    let host = local_host();
    let number = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) % 1000) as u16;
    let data_name = format!("dfA{number:03}{host}");
    let control_name = format!("cfA{number:03}{host}");
    let control = control_file(job, &host, &data_name);

    let mut s = target.connect(timeout)?;
    s.write_all(format!("\x02{}\n", target.path).as_bytes()).map_err(io(target))?;
    ack(&mut s, target, "receive job")?;

    s.write_all(format!("\x02{} {control_name}\n", control.len()).as_bytes()).map_err(io(target))?;
    ack(&mut s, target, "control file")?;
    s.write_all(control.as_bytes()).map_err(io(target))?;
    s.write_all(&[0]).map_err(io(target))?;
    ack(&mut s, target, "control file contents")?;

    s.write_all(format!("\x03{} {data_name}\n", data.len()).as_bytes()).map_err(io(target))?;
    ack(&mut s, target, "data file")?;
    s.write_all(data).map_err(io(target))?;
    s.write_all(&[0]).map_err(io(target))?;
    ack(&mut s, target, "data file contents")?;
    Ok(Receipt { protocol: Protocol::Lpd, job_id: Some(i32::from(number)), job_uri: None, state: None, ignored: Vec::new(), bytes: data.len() })
}

/// "Send queue state (short)" (RFC 1179 §5.3): the queue's own text, as its state message.
pub fn status(target: &Target, timeout: Duration) -> Result<PrinterStatus, PrintError> {
    let mut s = target.connect(timeout)?;
    s.write_all(format!("\x03{}\n", target.path).as_bytes()).map_err(io(target))?;
    let mut text = Vec::new();
    s.take(MAX_STATUS).read_to_end(&mut text).map_err(io(target))?;
    let text = String::from_utf8_lossy(&text);
    let message = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(20).collect::<Vec<_>>().join(" / ");
    let message: String = message.chars().filter(|c| !c.is_control()).take(500).collect();
    Ok(PrinterStatus {
        name: Some(target.path.clone()),
        state: Some("reachable".into()),
        state_message: Some(message).filter(|m| !m.is_empty()),
        ..Default::default()
    })
}
