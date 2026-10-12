//! Network printing against fake printers on the loopback interface: real TCP, real protocol
//! bytes, no hardware.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::ipp::{self, Request, Value, tag};
use super::*;
use crate::spool::Duplex;

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";
const T: Duration = Duration::from_secs(5);

/// One HTTP request as the fake printer saw it.
struct Seen {
    path: String,
    ipp: ipp::Response,
    body: Vec<u8>,
}

/// Read one HTTP POST: request line, headers, Content-Length body.
fn read_post(s: &mut BufReader<std::net::TcpStream>) -> (String, Vec<u8>) {
    let mut line = String::new();
    s.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        s.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some(v) = h.strip_prefix("Content-Length:") {
            len = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).unwrap();
    (path, body)
}

/// A fake IPP printer answering `n` requests. `formats` is its document-format-supported.
fn ipp_printer(n: usize, formats: &'static [&'static str], chunked: bool) -> (Target, mpsc::Receiver<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..n {
            let (stream, _) = l.accept().unwrap();
            let mut r = BufReader::new(stream);
            let (path, body) = read_post(&mut r);
            let req = ipp::parse(&body).unwrap();
            let op = req.status; // a request carries its operation where a reply has its status
            let mut resp = Request::new(0x0000, req.request_id);
            match op {
                ipp::op::GET_PRINTER_ATTRIBUTES => {
                    resp.group(tag::PRINTER);
                    resp.str(tag::NAME, "printer-name", "Office-Laser");
                    resp.str(tag::TEXT, "printer-make-and-model", "Acme LaserJet 9000");
                    resp.str(tag::TEXT, "printer-location", "2nd floor");
                    resp.int(tag::ENUM, "printer-state", 3);
                    resp.str(tag::KEYWORD, "printer-state-reasons", "toner-low-warning");
                    resp.boolean("printer-is-accepting-jobs", true);
                    resp.int(tag::INTEGER, "queued-job-count", 2);
                    let (first, rest) = formats.split_first().unwrap();
                    resp.str(tag::MIME, "document-format-supported", first);
                    for f in rest {
                        resp.also(tag::MIME, f);
                    }
                    resp.str(tag::KEYWORD, "sides-supported", "one-sided").also(tag::KEYWORD, "two-sided-long-edge");
                    resp.boolean("color-supported", false);
                    resp.str(tag::NAME, "marker-names", "Black Toner").also(tag::NAME, "Drum");
                    resp.int(tag::INTEGER, "marker-levels", 12).raw(tag::INTEGER, "", &77i32.to_be_bytes());
                    // A collection the client must skip without losing its place.
                    resp.bytes(tag::BEGIN_COLLECTION, "media-col-default", &[]);
                    resp.bytes(tag::MEMBER_NAME, "", b"media-size");
                    resp.bytes(tag::BEGIN_COLLECTION, "", &[]);
                    resp.bytes(tag::END_COLLECTION, "", &[]);
                    resp.bytes(tag::END_COLLECTION, "", &[]);
                    resp.str(tag::URI, "printer-uri-supported", "ipp://x/ipp/print");
                }
                ipp::op::PRINT_JOB => {
                    resp = Request::new(0x0001, req.request_id);
                    resp.group(tag::UNSUPPORTED);
                    resp.str(tag::KEYWORD, "finishings", "4");
                    resp.group(tag::JOB);
                    resp.int(tag::INTEGER, "job-id", 42);
                    resp.str(tag::URI, "job-uri", "ipp://x/jobs/42");
                    resp.int(tag::ENUM, "job-state", 4);
                }
                ipp::op::GET_JOBS => {
                    for (id, state) in [(7, 3), (8, 5)] {
                        resp.group(tag::JOB);
                        resp.int(tag::INTEGER, "job-id", id);
                        resp.str(tag::NAME, "job-name", "report.pdf");
                        resp.int(tag::ENUM, "job-state", state);
                        // force a fresh job group for the next job
                        resp.group(tag::OPERATION);
                    }
                }
                ipp::op::GET_JOB_ATTRIBUTES => {
                    resp.group(tag::JOB);
                    resp.int(tag::INTEGER, "job-id", 42);
                    resp.int(tag::ENUM, "job-state", 9);
                    resp.int(tag::INTEGER, "job-impressions-completed", 3);
                }
                _ => resp = Request::new(0x0501, req.request_id),
            }
            let body_out = resp.finish(&[]);
            let mut s = r.into_inner();
            if chunked {
                let (a, b) = body_out.split_at(body_out.len() / 2);
                write!(s, "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nTransfer-Encoding: chunked\r\n\r\n")
                    .unwrap();
                write!(s, "{:x}\r\n", a.len()).unwrap();
                s.write_all(a).unwrap();
                write!(s, "\r\n{:x};ext=1\r\n", b.len()).unwrap();
                s.write_all(b).unwrap();
                write!(s, "\r\n0\r\n\r\n").unwrap();
            } else {
                write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\n\r\n", body_out.len()).unwrap();
                s.write_all(&body_out).unwrap();
            }
            let doc = body.clone();
            tx.send(Seen { path, ipp: req, body: doc }).unwrap();
        }
    });
    (Target::parse(&format!("ipp://127.0.0.1:{port}/ipp/print")).unwrap(), rx)
}

fn op_str(r: &ipp::Response, group: u8, name: &str) -> Option<String> {
    r.get(group, name).and_then(|a| a.values.first()).and_then(Value::as_str).map(str::to_string)
}

#[test]
fn addresses_parse_in_every_form() {
    let t = Target::parse("ipp://10.0.0.5/ipp/print").unwrap();
    assert_eq!((t.protocol, t.host.as_str(), t.port, t.path.as_str()), (Protocol::Ipp, "10.0.0.5", 631, "/ipp/print"));
    assert_eq!(Target::parse("printer.local").unwrap().uri(), "ipp://printer.local/ipp/print");
    assert_eq!(Target::parse("http://printer:8631/printers/q").unwrap().uri(), "ipp://printer:8631/printers/q");
    let l = Target::parse("lpd://10.0.0.6/PASSTHRU").unwrap();
    assert_eq!((l.protocol, l.port, l.path.as_str()), (Protocol::Lpd, 515, "PASSTHRU"));
    assert_eq!(Target::parse("lpr://10.0.0.6").unwrap().path, "lp");
    let r = Target::parse("socket://[fe80::1]:9101").unwrap();
    assert_eq!((r.protocol, r.host.as_str(), r.port), (Protocol::Raw, "fe80::1", 9101));
    assert_eq!(r.uri(), "socket://[fe80::1]:9101");
    assert_eq!(Target::parse("raw://10.0.0.7").unwrap().port, 9100);
    assert_eq!(Target::parse("ipps://p/ipp/print").unwrap().protocol, Protocol::Ipps);
    // credentials are dropped, never sent
    assert_eq!(Target::parse("ipp://bob:secret@p/ipp").unwrap().host, "p");
    for bad in ["", "smb://p/q", "ipp://p:0/", "ipp://p:99999/", "ipp://", "ipp://[::1/x", "ipp://p q/", "lpd://p/a/b", "ipp://p;rm/"] {
        assert!(Target::parse(bad).is_err(), "{bad:?} should be refused");
    }
}

#[test]
fn ipp_messages_round_trip() {
    let mut r = Request::new(ipp::op::PRINT_JOB, 9);
    r.str(tag::URI, "printer-uri", "ipp://p/ipp/print");
    r.group(tag::JOB);
    r.int(tag::INTEGER, "copies", 3);
    r.str(tag::KEYWORD, "sides", "two-sided-long-edge").also(tag::KEYWORD, "one-sided");
    let msg = r.finish(b"DOC");
    assert!(msg.ends_with(b"\x03DOC"));
    let p = ipp::parse(&msg).unwrap();
    assert_eq!((p.status, p.request_id), (ipp::op::PRINT_JOB, 9));
    assert_eq!(op_str(&p, tag::OPERATION, "attributes-charset").as_deref(), Some("utf-8"));
    assert_eq!(p.get(tag::JOB, "copies").unwrap().values, vec![Value::Integer(3)]);
    assert_eq!(p.get(tag::JOB, "sides").unwrap().values.len(), 2);
}

#[test]
fn hostile_ipp_replies_are_errors_not_panics() {
    let good = {
        let mut r = Request::new(0, 1);
        r.group(tag::PRINTER);
        r.str(tag::TEXT, "printer-name", "x");
        r.finish(&[])
    };
    // Every truncation of a valid reply parses or fails cleanly.
    for n in 0..good.len() {
        let _ = ipp::parse(good.get(..n).unwrap());
    }
    // Lengths that claim more than is there.
    assert!(ipp::parse(&[1, 1, 0, 0, 0, 0, 0, 1, 4, 0x41, 0xFF, 0xFF, b'a']).is_err());
    // Integers of the wrong size.
    assert!(ipp::parse(&[1, 1, 0, 0, 0, 0, 0, 1, 4, 0x21, 0, 1, b'n', 0, 1, 7, 3]).is_err());
    // Collections nested beyond the limit.
    let mut deep = vec![1, 1, 0, 0, 0, 0, 0, 1, 4];
    for _ in 0..40 {
        deep.extend_from_slice(&[0x34, 0, 1, b'c', 0, 0]);
    }
    assert!(ipp::parse(&deep).is_err());
    // Too many attributes.
    let mut many = vec![1, 1, 0, 0, 0, 0, 0, 1, 4];
    for _ in 0..25_000 {
        many.extend_from_slice(&[0x21, 0, 1, b'n', 0, 4, 0, 0, 0, 1]);
    }
    assert!(ipp::parse(&many).is_err());
    // Random bytes never panic.
    let mut x: u32 = 0x1234_5678;
    for _ in 0..2_000 {
        let len = (x % 200) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        let _ = ipp::parse(&bytes);
        let _ = mdns::parse(&bytes);
    }
}

#[test]
fn printer_status_reports_state_supplies_and_formats() {
    let (t, rx) = ipp_printer(1, &["application/pdf", "image/urf"], false);
    let s = status(&t).unwrap();
    assert_eq!(s.name.as_deref(), Some("Office-Laser"));
    assert_eq!(s.make_and_model.as_deref(), Some("Acme LaserJet 9000"));
    assert_eq!(s.location.as_deref(), Some("2nd floor"));
    assert_eq!(s.state.as_deref(), Some("idle"));
    assert_eq!(s.state_reasons, vec!["toner-low-warning"]);
    assert_eq!(s.accepting_jobs, Some(true));
    assert_eq!(s.queued_jobs, Some(2));
    assert_eq!(s.color, Some(false));
    assert_eq!(s.sides, vec!["one-sided", "two-sided-long-edge"]);
    assert_eq!(s.supplies, vec![("Black Toner".to_string(), 12), ("Drum".to_string(), 77)]);
    assert!(s.takes_pdf());
    let seen = rx.recv_timeout(T).unwrap();
    assert_eq!(seen.path, "/ipp/print");
    assert_eq!(op_str(&seen.ipp, tag::OPERATION, "printer-uri").as_deref(), Some(t.uri().as_str()));
}

#[test]
fn a_print_job_carries_its_options_and_the_document() {
    let (t, rx) = ipp_printer(2, &["application/pdf"], true);
    let job = NetJob {
        copies: 3,
        collate: false,
        duplex: Some(Duplex::LongEdge),
        color: ColorMode::Monochrome,
        title: "Q3 \"report\".pdf".into(),
        user: "ana".into(),
        hold: false,
        pin: Some("4711".into()),
        account: Some("FIN-204".into()),
        finishings: vec![Finishing::Staple, Finishing::Punch],
        pjl: true,
    };
    let receipt = submit_with(&t, PDF, &job, T).unwrap();
    assert_eq!(receipt.job_id, Some(42));
    assert_eq!(receipt.job_uri.as_deref(), Some("ipp://x/jobs/42"));
    assert_eq!(receipt.state.as_deref(), Some("pending-held"));
    assert_eq!(receipt.ignored, vec!["finishings"]);

    let _attrs = rx.recv_timeout(T).unwrap(); // Get-Printer-Attributes first
    let seen = rx.recv_timeout(T).unwrap();
    let r = &seen.ipp;
    assert_eq!(r.status, ipp::op::PRINT_JOB);
    assert!(seen.body.ends_with(PDF), "the document follows the attributes");
    assert_eq!(op_str(r, tag::OPERATION, "requesting-user-name").as_deref(), Some("ana"));
    assert_eq!(op_str(r, tag::OPERATION, "job-name").as_deref(), Some("Q3 report.pdf"), "quotes are dropped");
    assert_eq!(op_str(r, tag::OPERATION, "document-format").as_deref(), Some("application/pdf"));
    assert_eq!(r.get(tag::OPERATION, "job-password").unwrap().values, vec![Value::Bytes(b"4711".to_vec())]);
    assert_eq!(r.get(tag::JOB, "copies").unwrap().values, vec![Value::Integer(3)]);
    assert_eq!(op_str(r, tag::JOB, "multiple-document-handling").as_deref(), Some("separate-documents-uncollated-copies"));
    assert_eq!(op_str(r, tag::JOB, "sides").as_deref(), Some("two-sided-long-edge"));
    assert_eq!(op_str(r, tag::JOB, "print-color-mode").as_deref(), Some("monochrome"));
    assert_eq!(op_str(r, tag::JOB, "job-hold-until").as_deref(), Some("indefinite"), "a PIN holds the job");
    assert_eq!(op_str(r, tag::JOB, "job-account-id").as_deref(), Some("FIN-204"));
    assert_eq!(r.get(tag::JOB, "finishings").unwrap().values, vec![Value::Enum(4), Value::Enum(5)]);
    assert_eq!(op_str(r, tag::JOB, "print-scaling").as_deref(), Some("none"));
}

#[test]
fn a_printer_that_cannot_read_pdf_is_refused_before_sending_anything() {
    let (t, rx) = ipp_printer(1, &["image/urf", "image/pwg-raster"], false);
    let e = submit_with(&t, PDF, &NetJob::default(), T).unwrap_err().to_string();
    assert!(e.contains("does not accept PDF") && e.contains("image/urf"), "{e}");
    rx.recv_timeout(T).unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "no Print-Job was sent");
}

#[test]
fn a_printer_that_sniffs_formats_gets_octet_stream() {
    let (t, rx) = ipp_printer(2, &["application/octet-stream", "application/postscript"], false);
    submit_with(&t, PDF, &NetJob::default(), T).unwrap();
    rx.recv_timeout(T).unwrap();
    let seen = rx.recv_timeout(T).unwrap();
    assert_eq!(op_str(&seen.ipp, tag::OPERATION, "document-format").as_deref(), Some("application/octet-stream"));
}

#[test]
fn jobs_are_listed_and_followed() {
    let (t, _rx) = ipp_printer(2, &["application/pdf"], false);
    let list = jobs(&t, false, None).unwrap();
    assert_eq!(list.iter().map(|j| (j.job_id, j.state.clone().unwrap())).collect::<Vec<_>>(), vec![(7, "pending".into()), (8, "processing".into())]);
    let j = job_status(&t, 42).unwrap();
    assert_eq!((j.job_id, j.state.as_deref(), j.sheets_completed), (42, Some("completed"), Some(3)));
}

#[test]
fn unsupported_operations_and_http_errors_explain_themselves() {
    let (t, _rx) = ipp_printer(1, &["application/pdf"], false);
    let e = cancel_job(&t, 42, "ana").unwrap_err().to_string();
    assert!(e.contains("server-error-operation-not-supported"), "{e}");

    // An HTTP 426 means the printer insists on TLS.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s);
        read_post(&mut r);
        r.into_inner().write_all(b"HTTP/1.1 426 Upgrade Required\r\nContent-Length: 0\r\n\r\n").unwrap();
    });
    let t = Target::parse(&format!("ipp://127.0.0.1:{port}/ipp/print")).unwrap();
    assert!(status(&t).unwrap_err().to_string().contains("TLS"));

    // ipps and job control on non-IPP printers are refused without connecting.
    assert!(submit(&Target::parse("ipps://192.0.2.1/").unwrap(), PDF, &NetJob::default()).unwrap_err().to_string().contains("ipps"));
    assert!(jobs(&Target::parse("socket://192.0.2.1").unwrap(), false, None).is_err());
}

#[test]
fn a_huge_or_garbled_http_reply_is_refused() {
    for reply in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\n"[..],
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        b"SSH-2.0-OpenSSH\r\n",
        b"",
    ] {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let reply = reply.to_vec();
        thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s);
            read_post(&mut r);
            r.into_inner().write_all(&reply).unwrap();
        });
        let t = Target::parse(&format!("ipp://127.0.0.1:{port}/ipp/print")).unwrap();
        assert!(status(&t).is_err());
    }
}

#[test]
fn bad_jobs_are_refused_before_connecting() {
    let t = Target::parse("socket://192.0.2.1").unwrap();
    for job in [
        NetJob { copies: 0, ..Default::default() },
        NetJob { copies: 1000, ..Default::default() },
        NetJob { pin: Some("12".into()), ..Default::default() },
        NetJob { pin: Some("12ab".into()), ..Default::default() },
    ] {
        assert!(matches!(submit(&t, PDF, &job), Err(crate::PrintError::Invalid(_))), "{job:?}");
    }
    assert_eq!(submit(&t, b"", &NetJob::default()), Err(crate::PrintError::NoPages));
}

#[test]
fn raw_jobs_go_out_wrapped_in_pjl() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).unwrap();
        tx.send(got).unwrap();
    });
    let t = Target::parse(&format!("socket://127.0.0.1:{port}")).unwrap();
    let job = NetJob {
        copies: 2,
        duplex: Some(Duplex::ShortEdge),
        color: ColorMode::Monochrome,
        title: "Contract".into(),
        user: "ana".into(),
        pin: Some("1234".into()),
        account: Some("LEGAL".into()),
        ..Default::default()
    };
    let receipt = submit_with(&t, PDF, &job, T).unwrap();
    let got = rx.recv_timeout(T).unwrap();
    assert_eq!(receipt.bytes, got.len());
    let text = String::from_utf8_lossy(&got);
    assert!(got.starts_with(pjl::UEL) && got.ends_with(pjl::UEL));
    for line in [
        "@PJL JOB NAME=\"Contract\"",
        "@PJL SET USERNAME=\"ana\"",
        "@PJL SET QTY=2",
        "@PJL SET DUPLEX=ON",
        "@PJL SET BINDING=SHORTEDGE",
        "@PJL SET RENDERMODE=GRAYSCALE",
        "@PJL SET JOBACCT=\"LEGAL\"",
        "@PJL SET HOLD=ON",
        "@PJL SET HOLDTYPE=PRIVATE",
        "@PJL SET HOLDKEY=\"1234\"",
        "@PJL ENTER LANGUAGE=PDF\r\n%PDF-1.7",
    ] {
        assert!(text.contains(line), "missing {line:?}");
    }

    // Without PJL the printer gets the PDF byte for byte.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut got = Vec::new();
        s.read_to_end(&mut got).unwrap();
        tx.send(got).unwrap();
    });
    let t = Target::parse(&format!("socket://127.0.0.1:{port}")).unwrap();
    submit_with(&t, PDF, &NetJob { pjl: false, ..Default::default() }, T).unwrap();
    assert_eq!(rx.recv_timeout(T).unwrap(), PDF);
}

/// A fake LPD queue that acknowledges each step and records the control and data files.
fn lpd_queue(refuse_queue: bool) -> (Target, mpsc::Receiver<(String, String, Vec<u8>)>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s);
        let mut line = Vec::new();
        r.read_until(b'\n', &mut line).unwrap();
        let queue = String::from_utf8_lossy(line.get(1..line.len() - 1).unwrap()).to_string();
        if refuse_queue {
            r.get_mut().write_all(&[1]).unwrap();
            return;
        }
        r.get_mut().write_all(&[0]).unwrap();
        let mut control = String::new();
        let mut data = Vec::new();
        for _ in 0..2 {
            line.clear();
            r.read_until(b'\n', &mut line).unwrap();
            let sub = line.first().copied().unwrap();
            let len: usize = String::from_utf8_lossy(&line[1..]).split_whitespace().next().unwrap().parse().unwrap();
            r.get_mut().write_all(&[0]).unwrap();
            let mut buf = vec![0u8; len + 1];
            r.read_exact(&mut buf).unwrap();
            assert_eq!(buf.pop(), Some(0));
            if sub == 2 {
                control = String::from_utf8(buf).unwrap()
            } else {
                data = buf
            }
            r.get_mut().write_all(&[0]).unwrap();
        }
        tx.send((queue, control, data)).unwrap();
    });
    (Target::parse(&format!("lpd://127.0.0.1:{port}/RAW")).unwrap(), rx)
}

#[test]
fn lpd_jobs_reach_the_queue() {
    let (t, rx) = lpd_queue(false);
    let job = NetJob { copies: 3, title: "Invoice 17".into(), user: "ana b".into(), pjl: false, ..Default::default() };
    let receipt = submit_with(&t, PDF, &job, T).unwrap();
    let (queue, control, data) = rx.recv_timeout(T).unwrap();
    assert_eq!(queue, "RAW");
    assert_eq!(data, PDF);
    assert!(control.contains("Pana_b\n") && control.contains("JInvoice 17\n"));
    assert_eq!(control.lines().filter(|l| l.starts_with('l')).count(), 3, "one print line per copy");
    assert_eq!(receipt.protocol, Protocol::Lpd);

    // With PJL the copies travel in the header and the queue prints the file once.
    let (t, rx) = lpd_queue(false);
    submit_with(&t, PDF, &NetJob { copies: 3, ..Default::default() }, T).unwrap();
    let (_, control, data) = rx.recv_timeout(T).unwrap();
    assert_eq!(control.lines().filter(|l| l.starts_with('l')).count(), 1);
    assert!(String::from_utf8_lossy(&data).contains("@PJL SET QTY=3"));

    let (t, _) = lpd_queue(true);
    let e = submit_with(&t, PDF, &NetJob::default(), T).unwrap_err().to_string();
    assert!(e.contains("refused") && e.contains("RAW"), "{e}");
}

#[test]
fn an_unreachable_printer_is_an_error_quickly() {
    // A port nothing listens on.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let t = Target::parse(&format!("socket://127.0.0.1:{port}")).unwrap();
    assert!(submit_with(&t, PDF, &NetJob::default(), Duration::from_secs(2)).unwrap_err().to_string().contains("cannot connect"));
}

/// Build a DNS response from records (no compression).
fn dns(records: &[(&str, u16, Vec<u8>)]) -> Vec<u8> {
    let name = |n: &str| {
        let mut v = Vec::new();
        for l in n.split('.') {
            v.push(l.len() as u8);
            v.extend_from_slice(l.as_bytes());
        }
        v.push(0);
        v
    };
    let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, records.len() as u8, 0, 0, 0, 0];
    for (n, t, rdata) in records {
        m.extend(name(n));
        m.extend_from_slice(&t.to_be_bytes());
        m.extend_from_slice(&[0, 1, 0, 0, 0, 120]);
        m.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        m.extend_from_slice(rdata);
    }
    m
}

fn encoded(n: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for l in n.split('.') {
        v.push(l.len() as u8);
        v.extend_from_slice(l.as_bytes());
    }
    v.push(0);
    v
}

#[test]
fn discovery_replies_become_printers() {
    let inst = "Office Laser._ipp._tcp.local";
    let mut srv = vec![0, 0, 0, 0, 0x02, 0x77]; // port 631
    srv.extend(encoded("laser.local"));
    let txt: Vec<u8> = ["rp=ipp/print", "ty=Acme LaserJet 9000", "note=2nd floor", "pdl=image/urf,application/pdf", "Color=F", "Duplex=T"]
        .iter()
        .flat_map(|s| std::iter::once(s.len() as u8).chain(s.bytes()))
        .collect();
    let mut raw_srv = vec![0, 0, 0, 0, 0x23, 0x8C]; // 9100
    raw_srv.extend(encoded("laser.local"));
    let msg = dns(&[
        ("_ipp._tcp.local", 12, encoded(inst)),
        (inst, 33, srv),
        (inst, 16, txt),
        ("laser.local", 28, vec![0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        ("laser.local", 1, vec![10, 0, 0, 5]),
        ("_pdl-datastream._tcp.local", 12, encoded("Office Laser._pdl-datastream._tcp.local")),
        ("Office Laser._pdl-datastream._tcp.local", 33, raw_srv),
        ("_ipp._tcp.local", 12, encoded("Lost._ipp._tcp.local")), // no SRV: skipped
    ]);
    let found = mdns::assemble(&mdns::parse(&msg));
    assert_eq!(found.len(), 2);
    let ipp = &found[0];
    assert_eq!(ipp.name, "Office Laser");
    assert_eq!(ipp.protocol, Protocol::Ipp);
    assert_eq!(ipp.uri(), "ipp://10.0.0.5/ipp/print", "IPv4 is preferred to link-local IPv6");
    assert_eq!(ipp.make_and_model.as_deref(), Some("Acme LaserJet 9000"));
    assert_eq!(ipp.location.as_deref(), Some("2nd floor"));
    assert_eq!((ipp.pdf, ipp.color, ipp.duplex), (Some(true), Some(false), Some(true)));
    assert_eq!(found[1].uri(), "socket://10.0.0.5");
}

#[test]
fn hostile_discovery_packets_are_harmless() {
    // A compression pointer pointing at itself.
    let mut m = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xC0, 12];
    m.extend_from_slice(&[0, 12, 0, 1, 0, 0, 0, 1, 0, 2, 0xC0, 12]);
    assert!(mdns::parse(&m).is_empty());
    // A record count far beyond the data.
    assert!(mdns::parse(&[0, 0, 0x84, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_empty());
    // A name longer than 255 bytes.
    let mut long = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    for _ in 0..10 {
        long.push(63);
        long.extend(std::iter::repeat_n(b'a', 63));
    }
    long.push(0);
    assert!(mdns::parse(&long).is_empty());
    // An SRV too short to hold a port.
    let msg = dns(&[("x._ipp._tcp.local", 33, vec![0, 0])]);
    assert!(mdns::parse(&msg).is_empty());
}

#[test]
fn queries_name_every_printer_service() {
    let q = mdns::query(7, &[("_ipp._tcp.local", 12)]);
    assert_eq!(&q[..2], &[0, 7]);
    assert!(q.windows(4).any(|w| w == b"_ipp"));
    assert!(q.ends_with(&[0, 0, 12, 0x80, 1]));
}

#[test]
fn pjl_strings_cannot_break_out_of_their_quotes() {
    let job = NetJob { title: "a\"\r\n@PJL SET X=1".into(), user: "\u{1b}e\"vil".into(), account: Some("\"".into()), ..Default::default() };
    let lines = pjl::header_lines(&job);
    assert!(lines.iter().all(|l| l.matches('"').count() % 2 == 0 && !l.contains('\r') && !l.contains('\u{1b}')));
    assert!(!lines.iter().any(|l| l.starts_with("@PJL SET JOBACCT")), "an empty account is left out");
}

/// A fake printer that answers IPP only on `path` (Canon's `/ipp/lp`), takes the formats given,
/// and shrugs off bare connections (a port check connects and closes).
fn picky_ipp_printer(path: &'static str, formats: &'static [&'static str]) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in l.incoming().take(32) {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(T));
            let mut r = BufReader::new(stream);
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let asked = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            if r.read_exact(&mut body).is_err() {
                continue;
            }
            let mut s = r.into_inner();
            if asked != path {
                let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                continue;
            }
            let Ok(req) = ipp::parse(&body) else { continue };
            let mut resp = Request::new(0x0000, req.request_id);
            resp.group(tag::PRINTER);
            resp.str(tag::NAME, "printer-name", "iR-ADV C5535");
            resp.str(tag::TEXT, "printer-make-and-model", "Canon iR-ADV C5535");
            resp.str(tag::TEXT, "printer-location", "Accounting");
            resp.int(tag::ENUM, "printer-state", 3);
            let (first, rest) = formats.split_first().unwrap();
            resp.str(tag::MIME, "document-format-supported", first);
            for f in rest {
                resp.also(tag::MIME, f);
            }
            let out = resp.finish(&[]);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\n\r\n", out.len());
            let _ = s.write_all(&out);
        }
    });
    port
}

fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn ranges_parse_and_big_ones_are_refused() {
    use std::net::Ipv4Addr;
    let r = parse_range("10.1.2.0/24").unwrap();
    assert_eq!((r.len(), r[0], r[253]), (254, Ipv4Addr::new(10, 1, 2, 1), Ipv4Addr::new(10, 1, 2, 254)));
    assert_eq!(parse_range("10.1.2.10-20").unwrap().len(), 11);
    assert_eq!(parse_range("10.1.2.250-10.1.3.5").unwrap().len(), 12);
    assert_eq!(parse_range(" 10.1.2.7 ").unwrap(), vec![Ipv4Addr::new(10, 1, 2, 7)]);
    assert_eq!(parse_range("10.1.2.7/32").unwrap().len(), 1);
    for bad in ["", "printer", "10.1.2.0/8", "10.1.2.0/33", "10.1.2.20-10", "10.0.0.0-10.1.0.0", "10.1.2.0/x", "1.2.3.4.5"] {
        assert!(parse_range(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn subnets_are_scanned_but_not_vpn_links_or_huge_networks() {
    use super::find::subnet_hosts;
    use std::net::Ipv4Addr;
    let ip = Ipv4Addr::new(192, 168, 5, 40);
    let hosts = subnet_hosts(ip, Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(hosts.len(), 253, "254 addresses less our own");
    assert!(!hosts.contains(&ip) && !hosts.contains(&Ipv4Addr::new(192, 168, 5, 255)));
    assert!(subnet_hosts(Ipv4Addr::new(10, 2, 0, 2), Ipv4Addr::new(255, 255, 255, 255)).is_empty(), "a VPN's /32");
    assert!(subnet_hosts(ip, Ipv4Addr::new(255, 255, 0, 0)).is_empty(), "a /16 is too big to scan unasked");
    assert_eq!(subnet_hosts(ip, Ipv4Addr::new(255, 255, 252, 0)).len(), 1021);
    assert!(subnet_hosts(ip, Ipv4Addr::new(255, 0, 255, 0)).is_empty(), "not a contiguous mask");
}

#[test]
fn ws_discovery_replies_are_recognised_without_trusting_them() {
    use super::find::is_printer_match;
    let reply = br#"<s:Envelope><s:Body><d:ProbeMatches><d:ProbeMatch><d:Types>wprt:PrintDeviceType</d:Types><d:XAddrs>http://10.9.9.9/wsd</d:XAddrs></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>"#;
    assert!(is_printer_match(reply));
    assert!(!is_printer_match(b"<Probe><Types>wprt:PrintDeviceType</Types></Probe>"), "our own probe echoed back");
    assert!(!is_printer_match(b"<ProbeMatch><Types>wscn:ScanDeviceType</Types></ProbeMatch>"));
    assert!(!is_printer_match(&[0xFF; 70_000]));
}

/// A Canon-style printer: Bonjour off, IPP only at /ipp/lp, no PDF without its option, raw 9100
/// open. A port check finds it, with its name, and says it doesn't take PDF.
#[test]
fn a_printer_without_bonjour_is_found_by_its_ports() {
    use std::net::Ipv4Addr;
    let ipp_port = picky_ipp_printer("/ipp/lp", &["application/vnd.canon-ufr2", "image/urf"]);
    let raw = TcpListener::bind("127.0.0.1:0").unwrap();
    let raw_port = raw.local_addr().unwrap().port();
    thread::spawn(move || for _ in raw.incoming().take(4) {});
    let found = super::find::probe_host_on(Ipv4Addr::LOCALHOST, [ipp_port, raw_port, closed_port()]);
    assert_eq!(found.len(), 2, "{found:?}");
    let ipp = &found[0];
    assert_eq!((ipp.protocol, ipp.path.as_str(), ipp.name.as_str()), (Protocol::Ipp, "/ipp/lp", "iR-ADV C5535"));
    assert_eq!(ipp.make_and_model.as_deref(), Some("Canon iR-ADV C5535"));
    assert_eq!(ipp.location.as_deref(), Some("Accounting"));
    assert_eq!(ipp.pdf, Some(false), "UFR II and URF only: needs the PDF Direct Print option");
    assert_eq!(ipp.uri(), format!("ipp://127.0.0.1:{ipp_port}/ipp/lp"));
    let rawf = &found[1];
    assert_eq!((rawf.protocol, rawf.port, rawf.name.as_str()), (Protocol::Raw, raw_port, "iR-ADV C5535 (port 9100)"));

    // Nothing listening: nothing found.
    assert!(super::find::probe_host_on(Ipv4Addr::LOCALHOST, [closed_port(), closed_port(), closed_port()]).is_empty());
}

/// The same printer with PDF Direct Print: it takes PDF.
#[test]
fn a_printer_that_lists_pdf_is_marked_so() {
    use std::net::Ipv4Addr;
    let ipp_port = picky_ipp_printer("/ipp/print", &["application/pdf", "image/urf"]);
    let found = super::find::probe_host_on(Ipv4Addr::LOCALHOST, [ipp_port, closed_port(), closed_port()]);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].path.as_str(), found[0].pdf), ("/ipp/print", Some(true)));
}
