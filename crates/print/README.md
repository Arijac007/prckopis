# pdfcraft-print

Layer L4: printing (execution plan M10.5).

```rust
let pages = select_pages(count, Some("1-3, ii, 9-"), &labels, Subset::All, false)?;
let settings = Settings { pages, paper: PAPERS[0].1, layout: Layout::multiple(4), ..Default::default() };
let sheets = layout(&display_sizes, &settings)?;   // geometry only (previews)
let pdf = impose(&doc, &settings)?;                // the print-ready PDF
spool::submit(&pdf, &spool::Job { printer: None, copies: 2, ..Default::default() })?;
```

- **Size**: fit (to the sheet minus an 18 pt margin), actual size, shrink oversized, custom %;
  centred; auto orientation turns the sheet for landscape pages.
- **Multiple**: 2/4/6/9/16 (or any n) pages per sheet, horizontal/vertical (reversed) order,
  page borders, auto-rotation of pages that don't match the cell.
- **Cut and stack** (Multiple's page order): consecutive pages in each cell's pile,
  with aligned cut marks in the gutters. For example, 10 pages at 4 per sheet give
  `[1, 4, 7, 10]`, `[2, 5, 8, blank]`, `[3, 6, 9, blank]`. Print single-sided,
  keep sheets in output order, cut at the marks, then stack the cell piles from
  left to right, top to bottom. Blank cells can be discarded. Page ranges and
  Reverse pages are applied before imposition. Duplex imposition is not supported.
  Headless: `doc_print` with `layout: "multiple"`, `order: "cut-stack"` and
  `per_sheet: 4` (or any supported grid size), plus `path` or `printer`.
- **Booklet**: saddle-stitch imposition padded to a multiple of 4; both sides, front or back
  only; left or right binding.
- **Poster**: tile scale, overlap shared by neighbouring tiles, cut marks.
- **Comments & forms**: document, + markups, + stamps, or form fields only; annotations print
  only with their Print flag, drawn from their appearance streams (Algorithm 8.1).

Each source page becomes a Form XObject (its content wrapped in q/Q, plus the printable
annotations); sheets place them with a clip. The result is a fresh, unencrypted,
garbage-collected file (callers check the print permission).

`spool` delegates discovery, driver options, preferences and submission to
[`craft-print`](https://github.com/storytold/craft-libs/tree/main/crates/print).
PdfCraft retains page selection, imposition, annotations and print-permission checks.
CUPS jobs stream through stdin with `fit-to-page=false`. Windows renders sheets through
`Windows.Data.Pdf` and spools them at their physical PDF size with per-page paper and
orientation, zero added margins and hardware-edge clipping. Long-edge and short-edge duplex
use the corresponding Windows binding edges. Random, exclusively created Windows job files
are removed on success and failure. Other targets offer the print-ready PDF instead.

The shared dependency requires Rust 1.95. Transport and native Windows regression tests
live in craft-libs; local tests retain composition, facade, engine, automation and UI coverage.
The desktop UI discovers printers and submits jobs on workers, so neither operation blocks
the dialog; the Windows backend starts without a console window. Physical printer output
still needs hardware validation.

In a browser there is no spooler: the app (`pdfcraft-ui-egui`'s `web_print`) draws the print-ready
PDF's sheets with PdfCraft's renderer and prints them through the browser's print dialog, which
sets copies, two-sided printing and colour. This crate stays free of web APIs.

Not yet: print as image, poster labels, PostScript output, colour conversion for
grayscale.

## Network printers (`net`)

Offices are full of printers nobody installed on the laptop in front of them. `net` sends the
print-ready PDF straight to them, without a system queue or a driver:

```rust
let target = net::Target::parse("ipp://10.0.0.5/ipp/print")?;   // or lpd://host/queue, socket://host:9100
let job = net::NetJob { copies: 2, duplex: Some(Duplex::LongEdge), color: ColorMode::Monochrome,
                        pin: Some("4711".into()), account: Some("FIN-204".into()), ..Default::default() };
let receipt = net::submit(&target, &pdf, &job)?;                 // job id and state from IPP
let found = net::discover()?;                                   // printers on the local network
```

| Protocol | Address | Options travel as | Status and jobs |
| --- | --- | --- | --- |
| IPP (RFC 8010/8011) over HTTP | `ipp://host[:631]/path` (also `http://`, or a bare host) | IPP job attributes | state, reasons, toner levels, formats; list, release and cancel jobs |
| LPD (RFC 1179) | `lpd://host[:515]/queue` | PJL header (or one print line per copy) | the queue's own text |
| Raw / JetDirect / AppSocket | `socket://host[:9100]` (also `raw://`) | PJL header | whether it answers |

- **Business options**: copies and collation, two-sided, colour or monochrome, the job's name
  and user (for the printer's job log), **held / secure printing** (`hold`, or a 4–10 digit
  `pin` released at the panel: IPP `job-hold-until` and `job-password`, PJL `HOLDTYPE=PRIVATE`
  with `HOLDKEY`), an **accounting / cost-centre code** (`job-account-id`, PJL `JOBACCT`), and
  finishing (staple, punch, fold).
- **Before sending**, an IPP printer is asked what it takes; one that reads no PDF (only
  `image/urf` or PWG raster) is refused with a message instead of printing pages of PDF source.
  A printer that sniffs formats gets `application/octet-stream`. Sheets go out with
  `print-scaling=none`: they are already laid out at their physical size.
- **Discovery** (`net::search`) combines three ways, because office printers are often
  invisible to Bonjour (many Canon imageRUNNER, Ricoh and Kyocera models ship with it off, and
  multicast never crosses VLANs):
  - legacy-unicast multicast-DNS queries for `_ipp._tcp`, `_ipps._tcp`, `_pdl-datastream._tcp`
    and `_printer._tcp`, sent through **every** IPv4 interface (a VPN that holds the default
    route no longer hides the office network), asking once more for what the first answers left
    out;
  - a WS-Discovery probe for `wprt:PrintDeviceType` (Windows' "WSD" printers), every interface;
  - a TCP check of 631 / 9100 / 515 on every address of the local subnets (up to 1,024 each,
    64 connections at a time) and of ranges the user names (`10.20.30.0/24`, `10.20.30.10-80`),
    asking IPP printers for name, model, location and formats at `/ipp/print`, `/ipp`, `/ipp/lp`
    (Canon), `/printer` and `/`.
  Printers that list no PDF format are kept and marked (`pdf: false`); the dialog explains they
  need their PDF option (Canon: PDF Direct Print) or the system driver. When nothing answers and
  a /32 tunnel interface is up, the dialog says a VPN may be sending the local network through
  its tunnel.
- **Untrusted input**: every reply is capped (HTTP headers 64 KiB, bodies 16 MiB, 20,000 IPP
  attributes, 2,000 DNS records), parsed with bounds checks, collections and DNS compression
  pointers are depth-limited, and PJL strings can't leave their quotes. Credentials in an address
  are dropped, never sent.
- Headless: `net_printers`, `net_printer_status`, `net_jobs`, `net_job_cancel`,
  `net_job_release`, and `doc_print` with `uri`. In the Print dialog: *Add network printer…*
  (type an address or *Find on network*), then Hold, Release PIN, Account code, Staple and
  *Printer status*.

Tests run against fake IPP, LPD and raw printers on the loopback interface. Not yet: `ipps`
(TLS) and HTTP sign-in (both refused with a clear message), raster-only IPP printers, remembering
added network printers across sessions, and validation on physical hardware.

## Architectural sheets and scale

The Paper menu and `doc_print` share Letter, Legal, Tabloid, A3/A4/A5 plus
ARCH A/B/C/D/E1/E, ANSI C/D/E and ISO A0/A1/A2. ARCH D is 24 × 36 inches;
ANSI D is 22 × 34 inches. Dimensions follow the [HP DesignJet paper-size table](https://support.hp.com/hr-en/document/bpp01676).
Automation accepts short names such as `"ARCH D"` (case-insensitive) or the full menu label.
For example, `doc_print` with `paper: "ARCH D"`, `layout: "actual"`,
`orientation: "auto"` and `path: "plan.pdf"` writes a print-ready PDF at 100%.

**Match page at 100%** uses the currently viewed document page to choose a standard
sheet (either orientation, within 0.5 pt for rounded metric dimensions), Actual size
and Auto orientation. It leaves the selected page range intact. It is disabled
for a custom page size with no matching preset. One preset applies to the whole job;
this is not per-page paper-source selection.

**PDF scale** describes the source-to-sheet transform in the previewed print-ready
PDF, not measured physical output. Fit reserves an 18 pt margin and can reduce even
a matching page. The current Windows backend still rasterises and can substitute
paper/shrink a job if its driver lacks the requested size. These controls do not
add #599's strict PrintTicket validation or vector spooler. Verify the driver's
paper/scaling settings and physical calibration before relying on drawing scale.
