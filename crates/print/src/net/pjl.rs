//! A PJL (Printer Job Language) job header around the document, for raw-socket and LPD jobs,
//! which have no other way to carry options. Office laser printers from most makers read PJL;
//! a printer that doesn't skips the header lines it doesn't understand.
//!
//! The header sets copies, collation, two-sided printing, colour, the job's name and user, an
//! accounting code, and held / PIN printing (`HOLD`, `HOLDTYPE=PRIVATE`, `HOLDKEY`).

use super::{ColorMode, Finishing, NetJob, clean_text};
use crate::spool::Duplex;

/// The Universal Exit Language sequence that starts and ends a PJL job.
pub const UEL: &[u8] = b"\x1b%-12345X";

/// The PJL lines for `job`, without the UEL and `ENTER LANGUAGE` lines.
pub fn header_lines(job: &NetJob) -> Vec<String> {
    let name = clean_text(&job.title, 80);
    let user = clean_text(&job.user, 40);
    let mut lines = vec![format!("@PJL JOB NAME=\"{name}\""), format!("@PJL SET JOBNAME=\"{name}\"")];
    if !user.is_empty() {
        lines.push(format!("@PJL SET USERNAME=\"{user}\""));
    }
    let copies = job.copies.clamp(1, 999);
    if job.collate {
        lines.push(format!("@PJL SET QTY={copies}"));
    } else {
        lines.push(format!("@PJL SET COPIES={copies}"));
    }
    match job.duplex {
        None => {}
        Some(Duplex::Off) => lines.push("@PJL SET DUPLEX=OFF".into()),
        Some(Duplex::LongEdge) => {
            lines.push("@PJL SET DUPLEX=ON".into());
            lines.push("@PJL SET BINDING=LONGEDGE".into());
        }
        Some(Duplex::ShortEdge) => {
            lines.push("@PJL SET DUPLEX=ON".into());
            lines.push("@PJL SET BINDING=SHORTEDGE".into());
        }
    }
    match job.color {
        ColorMode::Auto => {}
        ColorMode::Color => lines.push("@PJL SET RENDERMODE=COLOR".into()),
        ColorMode::Monochrome => lines.push("@PJL SET RENDERMODE=GRAYSCALE".into()),
    }
    if let Some(account) = &job.account {
        let account = clean_text(account, 40);
        if !account.is_empty() {
            lines.push(format!("@PJL SET JOBACCT=\"{account}\""));
        }
    }
    if job.finishings.contains(&Finishing::Staple) {
        lines.push("@PJL SET STAPLE=ON".into());
    }
    if job.finishings.contains(&Finishing::Punch) {
        lines.push("@PJL SET PUNCH=ON".into());
    }
    if job.held() {
        lines.push("@PJL SET HOLD=ON".into());
        match &job.pin {
            Some(pin) => {
                lines.push("@PJL SET HOLDTYPE=PRIVATE".into());
                lines.push(format!("@PJL SET HOLDKEY=\"{}\"", pin.chars().filter(char::is_ascii_digit).collect::<String>()));
            }
            None => lines.push("@PJL SET HOLDTYPE=PUBLIC".into()),
        }
    }
    lines
}

/// The document wrapped in a PJL job: UEL, header, `ENTER LANGUAGE=PDF`, the PDF, then the job's
/// end.
pub fn wrap(pdf: &[u8], job: &NetJob) -> Vec<u8> {
    let mut out = Vec::with_capacity(pdf.len().saturating_add(1024));
    out.extend_from_slice(UEL);
    out.extend_from_slice(b"@PJL\r\n");
    for line in header_lines(job) {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"@PJL ENTER LANGUAGE=PDF\r\n");
    out.extend_from_slice(pdf);
    out.extend_from_slice(UEL);
    out.extend_from_slice(format!("@PJL EOJ NAME=\"{}\"\r\n", clean_text(&job.title, 80)).as_bytes());
    out.extend_from_slice(UEL);
    out
}
