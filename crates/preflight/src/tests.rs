use super::*;

fn build(objs: &[&str], trailer_extra: &str) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(std::sync::Arc::new(out)).unwrap()
}

/// A page with embedded-font-free text, a note without the print flag, a link with a
/// JavaScript action, an interpolated image, and a document JavaScript.
fn messy() -> Document {
    build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript << /Names [(x) 9 0 R] >> >> /AA << /WC 9 0 R >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im 8 0 R >> >> /Annots [6 0 R 7 0 R] >>",
            "<< /Length 44 >>\nstream\n1 0 0 rg BT /F1 12 Tf 10 10 Td (Hi) Tj ET /Im Do\nendstream",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /P 3 0 R /AP << /N 10 0 R >> >>",
            "<< /Type /Annot /Subtype /Link /Rect [40 10 80 30] /P 3 0 R /F 4 /A 9 0 R >>",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate true /Length 1 >>\nstream\n\u{0}\nendstream",
            "<< /Type /Action /S /JavaScript /JS (app.alert(1)) >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream",
        ],
        "/Info << /Title (Report) /Producer (Test) /CreationDate (D:20240105093000Z) >>",
    )
}

fn clauses(issues: &[Issue]) -> Vec<&'static str> {
    let mut c: Vec<&str> = issues.iter().map(|i| i.clause).collect();
    c.dedup();
    c
}

#[test]
fn verify_reports_what_breaks_pdfa() {
    let issues = verify(&messy(), Level::A2b);
    let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    for want in [
        "The document has no XMP metadata",
        "Device colour is used, but there is no PDF/A output intent",
        "The document has additional actions (/AA)",
        "The document has document-level JavaScript",
        "A JavaScript action isn't allowed",
        "A Text annotation isn't set to print, or is hidden",
        "The font Helvetica isn't embedded",
        "An image asks to be interpolated",
    ] {
        assert!(msgs.contains(&want), "missing {want:?} in {msgs:#?}");
    }
    assert!(issues.iter().find(|i| i.message.starts_with("The font")).is_some_and(|i| !i.fixable));
    assert!(clauses(&issues).contains(&"6.6.2.1"));
    // Each problem is also worded separately from what the document calls things, so an interface
    // can say it in another language; in English the two come to the same sentence.
    for i in &issues {
        assert_eq!(i.said(&in_english), i.message);
        assert!(WORDINGS.contains(&i.pattern), "{:?} is not in WORDINGS", i.pattern);
    }
    let font = issues.iter().find(|i| i.pattern == "The font {} isn't embedded").unwrap();
    assert_eq!(font.said(&|w: &str| format!("«{w}»")), "«The font {} isn't embedded»".replace("{}", "Helvetica"));
}

/// Every problem the rules can report is listed in `WORDINGS`, so an interface showing them in
/// another language has something to translate. The wording is `verify`'s second argument, which is
/// what this reads off the file; the sentences `convert` reports as fixed are not wordings.
#[test]
fn wordings_are_complete() {
    let source = [include_str!("lib.rs"), include_str!("pdfx.rs")].concat();
    let source = source.as_str();
    let mut said = Vec::new();
    let mut rest = source;
    // `issue(` then the clause and the wording, as string literals, however rustfmt wraps them.
    while let Some(at) = rest.find("issue(") {
        rest = &rest[at + 6..];
        let Some(clause) = rest.trim_start().strip_prefix('"') else { continue };
        let Some(end) = clause.find('"') else { break };
        let Some(text) = clause[end + 1..].trim_start().strip_prefix(',').map(str::trim_start).and_then(|t| t.strip_prefix('"')) else {
            rest = clause;
            continue;
        };
        let Some(end) = text.find('"') else { break };
        said.push(&text[..end]);
        rest = text;
    }
    assert!(said.len() >= 24, "the wordings are not being read off the file: {said:#?}");
    let missing: Vec<&&str> = said.iter().filter(|word| !WORDINGS.contains(word)).collect();
    assert!(missing.is_empty(), "add these to WORDINGS, and a translation of each to every interface: {missing:#?}");
    let unsaid: Vec<&&str> = WORDINGS.iter().filter(|word| !said.contains(word)).collect();
    assert!(unsaid.is_empty(), "WORDINGS lists problems the rules no longer report: {unsaid:#?}");
}

#[test]
fn convert_fixes_all_but_fonts() {
    let mut doc = messy();
    let r = convert(&mut doc, Level::A2b).unwrap();
    assert!(r.fixed.iter().any(|f| f.contains("output intent")), "{:?}", r.fixed);
    let left: Vec<&str> = r.remaining.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(left, ["The font Helvetica isn't embedded"], "{:#?}", r.remaining);
    let d = declared(&doc);
    assert_eq!(d.pdfa, Some((2, "B".into())));
    assert_eq!(d.output_intents, ["sRGB IEC61966-2.1"]);
    // The metadata carries the document information.
    let x = metadata(&doc).unwrap();
    assert_eq!(xmp::value(&x, "dc:title").as_deref(), Some("Report"));
    assert_eq!(xmp::value(&x, "xmp:CreateDate").as_deref(), Some("2024-01-05T09:30:00Z"));
    // It survives a save.
    let bytes = prckopis_cos::write_full(&doc, &Default::default()).unwrap();
    let back = Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(verify(&back, Level::A2b).len(), 1);
}

#[test]
fn embedded_files_differ_between_parts() {
    let doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 4 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Filespec /F (a.txt) /EF << /F 5 0 R >> >>",
            "<< /Type /EmbeddedFile /Length 2 >>\nstream\nhi\nendstream",
        ],
        "",
    );
    assert!(verify(&doc, Level::A2b).iter().any(|i| i.clause == "6.8" && !i.fixable));
    let mut d3 = doc.clone();
    assert!(verify(&d3, Level::A3b).iter().any(|i| i.message.contains("AFRelationship")));
    let r = convert(&mut d3, Level::A3b).unwrap();
    assert!(r.remaining.is_empty(), "{:#?}", r.remaining);
}

/// A page that paints a DeviceCMYK image (as `doc_create` makes from a CMYK JPEG), with
/// `intent` as the catalog's extra entries and `extra` as more objects (from object 6 on).
fn cmyk_page(intent: &str, content: &str, extra: &[&str]) -> Document {
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R {intent} >>");
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let mut objs = vec![
        catalog.as_str(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>",
        stream.as_str(),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Length 4 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\nendstream",
    ];
    objs.extend_from_slice(extra);
    build(&objs, "")
}

fn messages(issues: &[Issue]) -> Vec<&str> {
    issues.iter().map(|i| i.message.as_str()).collect()
}

/// Issue #667: converting a CMYK-only document doesn't add an sRGB output intent (it would
/// describe a colour space the document doesn't use); the CMYK output intent it needs is
/// reported as not fixable instead.
#[test]
fn convert_does_not_give_cmyk_documents_an_srgb_output_intent() {
    let mut doc = cmyk_page("", "q 200 0 0 200 0 0 cm /Im0 Do Q", &[]);
    let r = convert(&mut doc, Level::A2b).unwrap();
    assert!(!r.fixed.iter().any(|f| f.contains("output intent")), "{:?}", r.fixed);
    assert!(declared(&doc).output_intents.is_empty());
    let left = messages(&r.remaining);
    assert!(left.contains(&"Device colour is used, but there is no PDF/A output intent"), "{left:#?}");
    assert!(left.contains(&"DeviceCMYK is used: it needs a CMYK output intent (not added automatically)"), "{left:#?}");
    assert!(r.remaining.iter().all(|i| !i.fixable));
}

/// Issue #667: verify checks the output intent's colour space against the device colour the
/// pages use, not only that an output intent exists.
#[test]
fn verify_reports_an_output_intent_that_does_not_match_device_colour() {
    let intent = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (X) /DestOutputProfile 6 0 R >>]";
    let profile = |n: u8| format!("<< /N {n} /Length 0 >>\nstream\n\nendstream");
    let (rgb, cmyk) = (profile(3), profile(4));
    let cmyk_msg = "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile";
    let rgb_msg = "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile";

    // CMYK content under an sRGB output intent (what convert used to write).
    let issues = verify(&cmyk_page(intent, "/Im0 Do", &[&rgb]), Level::A2b);
    assert!(messages(&issues).contains(&cmyk_msg), "{issues:#?}");
    assert!(issues.iter().find(|i| i.message == cmyk_msg).is_some_and(|i| i.clause == "6.2.4.3" && !i.fixable));

    // CMYK content under a CMYK output intent is fine; RGB and gray content beside it is not
    // and is, respectively.
    assert!(!messages(&verify(&cmyk_page(intent, "/Im0 Do 0 g", &[&cmyk]), Level::A2b)).iter().any(|m| m.contains("output intent")));
    let issues = verify(&cmyk_page(intent, "/Im0 Do 1 0 0 rg", &[&cmyk]), Level::A2b);
    assert!(messages(&issues).contains(&rgb_msg) && !messages(&issues).contains(&cmyk_msg), "{issues:#?}");

    // DefaultCMYK in the page's resources draws DeviceCMYK in that space, which PDF/A accepts
    // under any output intent.
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R {intent} >>");
    let doc = build(
        &[
            catalog.as_str(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ColorSpace << /DefaultCMYK [/ICCBased 7 0 R] >> /XObject << /Im0 5 0 R >> >> >>",
            "<< /Length 7 >>\nstream\n/Im0 Do\nendstream",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Length 4 >>\nstream\n\u{0}\u{0}\u{0}\u{0}\nendstream",
            rgb.as_str(),
            cmyk.as_str(),
        ],
        "",
    );
    assert!(!messages(&verify(&doc, Level::A2b)).contains(&cmyk_msg), "DefaultCMYK covers DeviceCMYK");

    // Without /N, the ICC header's colour space decides.
    let mut header = vec![b' '; 128];
    header[16..20].copy_from_slice(b"CMYK");
    let header = String::from_utf8(header).unwrap();
    let headed = format!("<< /Length 128 >>\nstream\n{header}\nendstream");
    assert!(!messages(&verify(&cmyk_page(intent, "/Im0 Do", &[&headed]), Level::A2b)).contains(&cmyk_msg));
}

// PDF/X-4 and Preflight profiles.

/// A 128-byte ICC header of device class `class` and colour space `space` (ICC.1 7.2.5, 7.2.6).
fn icc_header(class: &[u8; 4], space: &[u8; 4]) -> String {
    let mut h = vec![b' '; 128];
    h[12..16].copy_from_slice(class);
    h[16..20].copy_from_slice(space);
    String::from_utf8(h).unwrap()
}

const X4_XMP: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" \
pdfxid:GTS_PDFXVersion=\"PDF/X-4\" pdfaid:part=\"2\" pdfaid:conformance=\"B\"/></rdf:RDF></x:xmpmeta>";

/// A PDF/X-4 file: XMP identification, title, dates, Trapped, an ID, one CMYK printer output
/// intent, a TrimBox inside the BleedBox inside the MediaBox, CMYK content, a named optional
/// content configuration, and `page_extra`/`catalog_extra`/`objs` to break it with (objects 8
/// on).
fn x4(catalog_extra: &str, page_extra: &str, content: &str, objs: &[&str]) -> Document {
    let catalog = format!(
        "<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (FOGRA39) /DestOutputProfile 6 0 R >>] {catalog_extra} >>"
    );
    let page = format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /BleedBox [0 0 200 200] /TrimBox [10 10 190 190] {page_extra} >>");
    let stream = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    let meta = format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{X4_XMP}\nendstream", X4_XMP.len());
    let profile = format!("<< /N 4 /Length 128 >>\nstream\n{}\nendstream", icc_header(b"prtr", b"CMYK"));
    let mut all = vec![
        catalog.as_str(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
        page.as_str(),
        stream.as_str(),
        meta.as_str(),
        profile.as_str(),
        "<< /Title (Card) /CreationDate (D:20240105093000Z) /ModDate (D:20240105093000Z) /Trapped /False >>",
    ];
    all.extend_from_slice(objs);
    build(&all, "/Info 7 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>]")
}

#[test]
fn a_pdfx4_file_passes() {
    let doc = x4("", "", "0 0 0 1 k 0 0 100 100 re f", &[]);
    let issues = pdfx::verify(&doc);
    assert!(issues.is_empty(), "{issues:#?}");
    let r = profile::run(&doc, Profile::PdfX4);
    assert!(r.passed() && r.groups().is_empty());
}

/// Every problem the PDF/X-4 checks report on a file that isn't one, with its clause and whether
/// Fix can fix it.
#[test]
fn pdfx4_verify_reports_what_breaks_it() {
    let issues = pdfx::verify(&messy());
    let find = |m: &str| issues.iter().find(|i| i.message == m).unwrap_or_else(|| panic!("missing {m:?} in {issues:#?}"));
    for (m, clause, fixable) in [
        ("The document has no XMP metadata", "ISO 32000-1 14.3.2", true),
        ("The document has no modification date", "ISO 32000-1 14.3.3", true),
        ("The document doesn't say whether it is trapped (Trapped must be True or False)", "ISO 32000-1 14.11.6", true),
        ("The file has no identifier (ID)", "ISO 32000-1 14.4", true),
        ("There is no PDF/X output intent", "ISO 32000-1 14.11.5", false),
        ("The page has neither a TrimBox nor an ArtBox", "ISO 32000-1 14.11.2", true),
        ("The font Helvetica isn't embedded", "ISO 32000-1 9.9", false),
        ("A Link annotation prints inside the bleed area", "ISO 32000-1 12.5.3", false),
    ] {
        let i = find(m);
        assert_eq!((i.standard, i.clause, i.fixable), (pdfx::STANDARD, clause, fixable), "{m}");
        assert!(WORDINGS.contains(&i.pattern), "{:?} is not in WORDINGS", i.pattern);
        assert_eq!(i.said(&in_english), i.message);
    }
    assert_eq!(find("The page has neither a TrimBox nor an ArtBox").page, Some(0));
    assert_eq!(find("The file has no identifier (ID)").citation(), "ISO 15930-7 · ISO 32000-1 14.4");
    // The title and creation date are there; a Text note without the print flag doesn't print.
    assert!(!issues.iter().any(|i| i.message.contains("title") || i.message.contains("creation date") || i.message.contains("Text annotation")));
}

#[test]
fn pdfx4_output_intent_rules() {
    let msgs = |d: &Document| pdfx::verify(d).into_iter().map(|i| i.message).collect::<Vec<_>>();
    // Two PDF/X output intents.
    let two = x4("/Extra 1", "", "0 0 0 1 k", &[]);
    let mut two = two;
    let root = two.root().unwrap();
    two.update_dict(root, |c| {
        let mut list = c.get(b"OutputIntents").unwrap().as_array().unwrap().clone();
        list.push(list[0].clone());
        c.set(b"OutputIntents".to_vec(), Object::Array(list));
    })
    .unwrap();
    assert!(msgs(&two).contains(&"There is more than one PDF/X output intent".to_owned()));
    // A monitor profile, no condition identifier, RGB content under a CMYK intent.
    let doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /S /GTS_PDFX /DestOutputProfile 5 0 R >>] >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /TrimBox [0 0 200 200] >>",
            "<< /Length 8 >>\nstream\n1 0 0 rg\nendstream",
            &format!("<< /N 4 /Length 128 >>\nstream\n{}\nendstream", icc_header(b"mntr", b"CMYK")),
        ],
        "",
    );
    let m = msgs(&doc);
    for want in [
        "The PDF/X output intent has no output condition identifier",
        "The PDF/X output intent's profile isn't an output (printer) profile",
        "DeviceRGB is used, but the PDF/X output intent isn't an RGB profile",
    ] {
        assert!(m.contains(&want.to_owned()), "missing {want:?} in {m:#?}");
    }
    assert!(!m.iter().any(|x| x.starts_with("DeviceCMYK")), "{m:#?}");
    // DefaultRGB characterises DeviceRGB, which a CMYK output intent then accepts.
    let ok = x4("", "/Resources << /ColorSpace << /DefaultRGB [/ICCBased 6 0 R] >> >>", "1 0 0 rg", &[]);
    assert!(!msgs(&ok).iter().any(|x| x.starts_with("DeviceRGB")), "{:#?}", msgs(&ok));
    // An output intent without a profile; CMYK content under an RGB one.
    let none = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /S /GTS_PDFX /OutputConditionIdentifier (x) >>] >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /TrimBox [0 0 200 200] >>",
        ],
        "",
    );
    assert!(msgs(&none).contains(&"The PDF/X output intent has no destination profile".to_owned()));
    let rgb_intent = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /S /GTS_PDFX /OutputConditionIdentifier (x) /DestOutputProfile 5 0 R >>] >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /TrimBox [0 0 200 200] >>",
            "<< /Length 9 >>\nstream\n0 0 0 1 k\nendstream",
            &format!("<< /Length 128 >>\nstream\n{}\nendstream", icc_header(b"prtr", b"RGB ")),
        ],
        "",
    );
    let m = msgs(&rgb_intent);
    assert!(m.contains(&"DeviceCMYK is used, but the PDF/X output intent isn't a CMYK profile".to_owned()), "{m:#?}");
    assert!(!m.iter().any(|x| x.contains("printer")), "{m:#?}");
}

#[test]
fn pdfx4_page_box_rules_and_grouping() {
    let doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /TrimBox [10 10 190 190] /ArtBox [20 20 180 180] >>",
            "<< /Type /Page /Parent 2 0 R /BleedBox [-5 -5 205 205] /TrimBox [-6 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /BleedBox [10 10 100 100] /TrimBox [0 0 200 200] >>",
        ],
        "",
    );
    let r = profile::run(&doc, Profile::PdfX4);
    let on = |m: &str| r.issues.iter().filter(|i| i.message == m).map(|i| i.page.unwrap()).collect::<Vec<_>>();
    assert_eq!(on("The page has both a TrimBox and an ArtBox"), [0]);
    assert_eq!(on("A page box extends beyond the MediaBox"), [1]);
    assert_eq!(on("The TrimBox extends beyond the BleedBox"), [1, 2]);
    assert!(on("The page has neither a TrimBox nor an ArtBox").is_empty());
    // The same problem on two pages is one row with both pages.
    let g = r.groups().into_iter().find(|g| g.issue.message == "The TrimBox extends beyond the BleedBox").unwrap();
    assert_eq!((g.count, g.pages.as_slice(), g.issue.fixable), (2, &[1, 2][..], false));

    let mut fixed = doc.clone();
    let f = profile::fix(&mut fixed, Profile::PdfX4, Some("D:20261013120000Z")).unwrap();
    let pages = prckopis_model::pages(&fixed);
    assert!(pages[0].dict.get(b"ArtBox").is_none() && pages[0].dict.get(b"TrimBox").is_some());
    let bleed: Vec<f64> = pages[1].dict.get(b"BleedBox").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(bleed, [0.0, 0.0, 200.0, 200.0]);
    // Clipping both boxes to the MediaBox put page 2's TrimBox inside its BleedBox; page 3's
    // needs a decision about the trim, which Fix doesn't make.
    let left: Vec<Option<usize>> = f.results.issues.iter().filter(|i| i.clause == "ISO 32000-1 14.11.2").map(|i| i.page).collect();
    assert_eq!(left, [Some(2)], "{:#?}", f.results.issues);
}

/// Fix fixes everything it says it can and nothing else, keeps the file's PDF/A identification,
/// and what it writes survives a save.
#[test]
fn pdfx4_fix_fixes_what_does_not_change_print() {
    let old_xmp = "<x:xmpmeta><rdf:RDF><rdf:Description pdfaid:part=\"2\" pdfaid:conformance=\"B\" dc:format=\"x\"/></rdf:RDF></x:xmpmeta>";
    let meta = format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{old_xmp}\nendstream", old_xmp.len());
    let mut doc = build(
        &[
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [8 0 R] /D << /Order [8 0 R] /AS [] >> /Configs [9 0 R] >> /Metadata 5 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] /CropBox [5 5 195 195] >>",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im 7 0 R >> >> >>",
            "<< /Length 7 >>\nstream\n/Im0 Do\nendstream",
            meta.as_str(),
            "<< /Title (Poster) >>",
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Alternates [] /OPI << >> /Length 1 >>\nstream\n\u{0}\nendstream",
            "<< /Type /OCG /Name (Layer) >>",
            "<< /AS [] >>",
        ],
        "/Info 6 0 R",
    );
    let before = pdfx::verify(&doc);
    let f = profile::fix(&mut doc, Profile::PdfX4, Some("D:20261013120000Z")).unwrap();
    // Only the output intent, which needs the user's printing condition, is left.
    let left: Vec<&str> = f.results.issues.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(left, ["There is no PDF/X output intent"], "fixed {:?}", f.fixed);
    assert!(before.iter().filter(|i| i.fixable).count() >= 8, "{before:#?}");
    for i in &f.results.issues {
        assert!(!i.fixable, "{i:?} was fixable but is still there");
    }
    let saved = prckopis_cos::write_full(&doc, &Default::default()).unwrap();
    let back = Document::open(std::sync::Arc::new(saved)).unwrap();
    let x = metadata(&back).unwrap();
    assert_eq!(xmp::value(&x, "pdfxid:GTS_PDFXVersion").as_deref(), Some("PDF/X-4"));
    assert_eq!(xmp::value(&x, "pdfaid:part").as_deref(), Some("2"), "the PDF/A identification stays: {x}");
    assert_eq!(xmp::value(&x, "pdf:Trapped").as_deref(), Some("False"));
    assert_eq!(xmp::value(&x, "dc:title").as_deref(), Some("Poster"));
    assert_eq!(xmp::value(&x, "xmp:CreateDate").as_deref(), Some("2026-10-13T12:00:00Z"));
    assert_eq!(info(&back, "GTS_PDFXVersion").as_deref(), Some("PDF/X-4"));
    let trim: Vec<f64> =
        prckopis_model::pages(&back)[0].dict.get(b"TrimBox").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(trim, [5.0, 5.0, 195.0, 195.0], "the trim box is the crop box");
    assert_eq!(pdfx::verify(&back).len(), 1);

    // Without a clock the dates stay missing, and say so.
    let mut no_clock = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>"], "");
    let f = profile::fix(&mut no_clock, Profile::PdfX4, None).unwrap();
    assert!(f.results.issues.iter().any(|i| i.message == "The document has no creation date"), "{:#?}", f.results.issues);
}

/// Save as PDF/A keeps what the old XMP said about PDF/X and trapping.
#[test]
fn convert_to_pdfa_keeps_the_pdfx_identification() {
    let mut doc = x4("", "", "0 0 0 1 k", &[]);
    convert(&mut doc, Level::A2b).unwrap();
    let x = metadata(&doc).unwrap();
    assert_eq!(xmp::value(&x, "pdfxid:GTS_PDFXVersion").as_deref(), Some("PDF/X-4"));
    assert_eq!(xmp::value(&x, "pdfaid:part").as_deref(), Some("2"));
    assert!(pdfx::verify(&doc).is_empty(), "{:#?}", pdfx::verify(&doc));
}

#[test]
fn pdfx4_graphics_states_and_optional_content() {
    let doc = x4(
        "/OCProperties << /OCGs [] /D << /Name (Main) >> /Configs [<< /AS [] /Name (Print) >> << >>] >>",
        "/Resources << /ExtGState << /G0 << /TR /Identity >> /G1 8 0 R >> >>",
        "0 0 0 1 k",
        &["<< /Type /ExtGState /TR2 /Default >>"],
    );
    let m: Vec<String> = pdfx::verify(&doc).into_iter().map(|i| i.message).collect();
    assert_eq!(
        m,
        [
            "A graphics state sets a transfer function",
            "An optional content configuration has an AS entry",
            "An optional content configuration has no name"
        ]
    );
    let mut fixed = doc.clone();
    let f = profile::fix(&mut fixed, Profile::PdfX4, None).unwrap();
    let left: Vec<&str> = f.results.issues.iter().map(|i| i.message.as_str()).collect();
    assert_eq!(left, ["A graphics state sets a transfer function"], "{:?}", f.fixed);
}

#[test]
fn profiles_by_id_run_and_report() {
    assert_eq!(Profile::from_id("pdfx-4"), Some(Profile::PdfX4));
    assert_eq!(Profile::from_id("PDF/X-4"), Some(Profile::PdfX4));
    assert_eq!(Profile::from_id("pdfa-3b"), Some(Profile::PdfA3b));
    assert_eq!(Profile::from_id("2b"), Some(Profile::PdfA2b));
    assert_eq!(Profile::from_id("x9"), None);
    for p in Profile::ALL {
        assert_eq!(Profile::from_id(p.id()), Some(p));
    }
    let doc = messy();
    assert_eq!(profile::run(&doc, Profile::PdfA2b).issues, verify(&doc, Level::A2b));
    assert!(profile::run(&doc, Profile::PdfA3b).issues.iter().all(|i| i.standard == "ISO 19005-3"));

    let r = profile::run(&doc, Profile::PdfX4);
    let text = report_text(&r, "messy.pdf", "2026-10-13", &in_english);
    for want in [
        "Preflight report",
        "Profile: Verify compliance with PDF/X-4",
        "File: messy.pdf",
        "Date: 2026-10-13",
        "Pages: 1",
        "ISO 15930-7 · ISO 32000-1 14.11.2 · Can be fixed",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    assert!(text.contains(&format!("Problems found: {}", r.issues.len())));
    // Every word of the report is ours to translate, and goes through `words`.
    let shouted = report_text(&r, "m.pdf", "d", &|w: &str| w.to_uppercase());
    assert!(shouted.contains("PREFLIGHT REPORT") && shouted.contains("PROFILE: VERIFY COMPLIANCE WITH PDF/X-4"), "{shouted}");
    let clean = x4("", "", "0 0 0 1 k", &[]);
    let ok = report_text(&profile::run(&clean, Profile::PdfX4), "ok.pdf", "d", &in_english);
    assert!(ok.ends_with("No problems found.\n"), "{ok}");
}

/// Broken files: wrong types everywhere the checks and fixups look. Nothing panics.
#[test]
fn malformed_pdfx_input_never_panics() {
    let objs: &[&[&str]] = &[
        &[
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents 7 /OCProperties << /D 5 /Configs [99 0 R (x) << /Name 3 >>] >> /Metadata 4 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 nan] >>",
            "<< /Type /Page /Parent 2 0 R /TrimBox [1 2 3] /ArtBox (x) /BleedBox [1e308 -1e308 1e308 1e308] /Annots [5 0 R 99 0 R 7] /Resources << /ExtGState 9 >> >>",
            "<< /Length 5 >>\nstream\n<\u{0}x>\nendstream",
            "<< /Type /Annot /Subtype /Square /F 4 /Rect [0 0] >>",
        ],
        &[
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /S /GTS_PDFX /DestOutputProfile << /N 4 >> >> 7 << /S /GTS_PDFX /DestOutputProfile 4 0 R /OutputConditionIdentifier 5 >>] >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [200 200 0 0] /CropBox [500 500 600 600] /TrimBox [-10 -10 -5 -5] >>",
            "<< /N (four) /Length 3 >>\nstream\nabc\nendstream",
        ],
    ];
    for (n, set) in objs.iter().enumerate() {
        for trailer in ["", "/Info 7 /ID (x)", "/Info 3 0 R /ID [1 2]", "/Info << /Trapped /Maybe /Title 5 >> /ID [(a)]"] {
            let doc = build(set, trailer);
            let _ = pdfx::verify(&doc);
            let r = profile::run(&doc, Profile::PdfX4);
            let _ = report_text(&r, "", "", &in_english);
            for p in Profile::ALL {
                let mut d = doc.clone();
                let _ = profile::fix(&mut d, p, Some("D:2026"));
            }
            let mut d = doc.clone();
            let f = profile::fix(&mut d, Profile::PdfX4, None).unwrap_or_else(|e| panic!("set {n} {trailer}: {e}"));
            assert!(
                f.results.issues.iter().any(|i| i.message == "There is no PDF/X output intent" || i.message.contains("output intent")),
                "set {n}"
            );
        }
    }
}
