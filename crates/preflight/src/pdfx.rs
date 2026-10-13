//! PDF/X-4 (ISO 15930-7:2010): the requirements that can be decided from the object graph, and the
//! fixups that don't change what prints.
//!
//! PDF/X-4 restricts objects that ISO 32000-1 defines, so each check cites the ISO 32000-1 clause
//! that defines what it inspects, with the standard ([`STANDARD`]) the requirement comes from:
//!
//! - identification: XMP metadata (14.3.2) with `pdfxid:GTS_PDFXVersion` `PDF/X-4`; a title and the
//!   creation and modification dates (14.3.3); `Trapped` `True` or `False`, not `Unknown`
//!   (14.3.3, 14.11.6); a file identifier (14.4); no encryption (7.6);
//! - exactly one PDF/X output intent (`GTS_PDFX`) with an output condition identifier and an
//!   embedded output (`prtr`) destination profile (14.11.5); DeviceRGB only with an RGB and
//!   DeviceCMYK only with a CMYK output intent, unless a `DefaultRGB`/`DefaultCMYK` colour space
//!   characterises it (8.6.5.6);
//! - pages: a TrimBox or an ArtBox but not both, every box within the MediaBox and the TrimBox
//!   within the BleedBox (14.11.2); no printing annotation inside the bleed area other than
//!   TrapNet and PrinterMark (12.5.3, 12.5.6.20–21);
//! - every font embedded (9.9); no external streams (7.3.8.2); no PostScript XObjects (8.8.2); no
//!   image alternates (8.9.5.4) or OPI (14.11.7); no transfer functions (8.4.5); optional
//!   content configurations named and without `AS` (8.11.4.3).
//!
//! Not checked yet: the PDF version, transparency group colour spaces, Separation and DeviceN
//! alternates, halftones, 16-bit images, and the `DestOutputProfileRef` of PDF/X-4p.
//!
//! [`fix`] identifies the file in its XMP and document information, sets `Trapped` to `False`
//! where it is unknown, fills in missing dates from the time it is given, gives the file an
//! identifier, removes encryption, image alternates and OPI, adds a TrimBox (the crop box) where a
//! page has neither box, drops an ArtBox that sits beside a TrimBox, clips boxes to the MediaBox,
//! and names optional content configurations and removes their `AS`. It never changes what a page
//! looks like: an output intent, a missing font, device colour or a printing annotation in the
//! bleed is left to the user.

use std::collections::HashSet;

use prckopis_cos::{CosError, Dict, Document, Object, PdfString, Stream};

use crate::{Issue, catalog, check_font, dict_of, info, metadata, objects, output_intents, profile_components, uses_device_colour, xmp};

/// The standard the checks come from.
pub const STANDARD: &str = "ISO 15930-7";

/// What `pdfxid:GTS_PDFXVersion` says for a PDF/X-4 file.
pub const VERSION: &str = "PDF/X-4";

/// How far a box may stick out of another before it counts (points): rounding in writers.
const SLACK: f64 = 0.01;

/// A rectangle `[llx lly urx ury]`, normalised, or `None` when it isn't four finite numbers.
fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        [a, b, c, d] if v.iter().all(|x| x.is_finite()) => Some([a.min(*c), b.min(*d), a.max(*c), b.max(*d)]),
        _ => None,
    }
}

fn inside(inner: [f64; 4], outer: [f64; 4]) -> bool {
    inner[0] >= outer[0] - SLACK && inner[1] >= outer[1] - SLACK && inner[2] <= outer[2] + SLACK && inner[3] <= outer[3] + SLACK
}

fn intersection(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

fn rect_object(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// A page's boxes, with the defaults ISO 32000-1 14.11.2 gives the ones it leaves out.
struct Boxes {
    media: [f64; 4],
    /// The crop box clipped to the media box.
    crop: [f64; 4],
    bleed: Option<[f64; 4]>,
    trim: Option<[f64; 4]>,
    art: Option<[f64; 4]>,
}

impl Boxes {
    fn of(doc: &Document, page: &Dict) -> Boxes {
        // US Letter when the MediaBox is missing or broken, as viewers assume.
        let media = rect(doc, page.get(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let crop = rect(doc, page.get(b"CropBox")).and_then(|c| intersection(c, media)).unwrap_or(media);
        Boxes { media, crop, bleed: rect(doc, page.get(b"BleedBox")), trim: rect(doc, page.get(b"TrimBox")), art: rect(doc, page.get(b"ArtBox")) }
    }

    /// The bleed area: the BleedBox, else the crop box (14.11.2).
    fn bleed_area(&self) -> [f64; 4] {
        self.bleed.and_then(|b| intersection(b, self.crop)).unwrap_or(self.crop)
    }
}

/// Whether the trailer has a file identifier: two strings (ISO 32000-1 14.4).
fn has_id(doc: &Document) -> bool {
    match doc.trailer().get(b"ID").map(|i| doc.resolve(i)) {
        Some(a) => a.as_array().is_some_and(|a| a.len() == 2 && a.iter().all(|s| doc.resolve(s).as_string().is_some())),
        None => false,
    }
}

/// `True` or `False` from the document information's `Trapped` (a name; PDF 1.3 writers also
/// wrote a boolean or a string), or from the XMP's `pdf:Trapped`.
fn trapped(doc: &Document, xmp_text: Option<&str>) -> Option<&'static str> {
    let from_info =
        doc.trailer().get(b"Info").map(|i| doc.resolve(i)).and_then(|i| i.as_dict().and_then(|d| d.get(b"Trapped").map(|t| doc.resolve(t))));
    let said = from_info.and_then(|t| match &*t {
        Object::Name(n) if n == b"True" => Some("True"),
        Object::Name(n) if n == b"False" => Some("False"),
        Object::Bool(true) => Some("True"),
        Object::Bool(false) => Some("False"),
        Object::String(s) if s.to_text() == "True" => Some("True"),
        Object::String(s) if s.to_text() == "False" => Some("False"),
        _ => None,
    });
    said.or_else(|| xmp::Ids::declared_in(xmp_text).trapped)
}

/// Every graphics state parameter dictionary: indirect ones and those inside resource dictionaries.
fn graphics_states(doc: &Document, objs: &[(prckopis_cos::ObjRef, std::sync::Arc<Object>)]) -> Vec<Dict> {
    let mut out = Vec::new();
    for (_, o) in objs {
        let Some(d) = dict_of(o) else { continue };
        if d.name(b"Type") == Some(b"ExtGState") {
            out.push(d.clone());
        }
        let resources = d.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        for holder in [Some(d), resources.as_ref()].into_iter().flatten() {
            let Some(states) = holder.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()) else { continue };
            for (_, gs) in states.iter() {
                // Indirect ones were seen above if they say /Type /ExtGState; seeing one twice
                // only repeats an issue, which `verify` folds into one.
                if let Some(g) = doc.resolve(gs).as_dict() {
                    out.push(g.clone());
                }
            }
        }
    }
    out
}

/// The optional content configuration dictionaries: `/D` and each of `/Configs` (8.11.4.3).
fn oc_configs(doc: &Document) -> Vec<(Option<prckopis_cos::ObjRef>, Dict)> {
    let cat = catalog(doc);
    let Some(props) = cat.get(b"OCProperties").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut take = |o: &Object| {
        if let Some(d) = doc.resolve(o).as_dict() {
            out.push((o.as_ref(), d.clone()));
        }
    };
    if let Some(d) = props.get(b"D") {
        take(d);
    }
    if let Some(list) = props.get(b"Configs").map(|c| doc.resolve(c)).and_then(|c| c.as_array().cloned()) {
        list.iter().for_each(&mut take);
    }
    out
}

/// Check `doc` against PDF/X-4.
pub fn verify(doc: &Document) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut issue = |clause: &'static str, pattern: &'static str, value: Option<String>, page: Option<usize>, fixable: bool| {
        out.push(Issue::new(STANDARD, clause, pattern, value, page, fixable));
    };
    // Identification and the document information (14.3).
    let xmp_text = metadata(doc);
    match &xmp_text {
        None => issue("ISO 32000-1 14.3.2", "The document has no XMP metadata", None, None, true),
        Some(x) => {
            if !xmp::value(x, "pdfxid:GTS_PDFXVersion").is_some_and(|v| v.trim().starts_with(VERSION)) {
                issue("ISO 32000-1 14.3.2", "The metadata doesn't identify the file as {}", Some(VERSION.to_owned()), None, true);
            }
        }
    }
    let from_xmp = |key: &str| xmp_text.as_deref().and_then(|x| xmp::value(x, key)).is_some_and(|v| !v.trim().is_empty());
    if info(doc, "Title").is_none_or(|t| t.trim().is_empty()) && !from_xmp("dc:title") {
        issue("ISO 32000-1 14.3.3", "The document has no title", None, None, false);
    }
    if info(doc, "CreationDate").is_none_or(|d| d.trim().is_empty()) && !from_xmp("xmp:CreateDate") {
        issue("ISO 32000-1 14.3.3", "The document has no creation date", None, None, true);
    }
    if info(doc, "ModDate").is_none_or(|d| d.trim().is_empty()) && !from_xmp("xmp:ModifyDate") {
        issue("ISO 32000-1 14.3.3", "The document has no modification date", None, None, true);
    }
    if trapped(doc, xmp_text.as_deref()).is_none() {
        issue("ISO 32000-1 14.11.6", "The document doesn't say whether it is trapped (Trapped must be True or False)", None, None, true);
    }
    if !has_id(doc) {
        issue("ISO 32000-1 14.4", "The file has no identifier (ID)", None, None, true);
    }
    if doc.trailer().get(b"Encrypt").is_some() {
        issue("ISO 32000-1 7.6", "The document is encrypted", None, None, true);
    }
    // The output intent (14.11.5) and the device colour it characterises (8.6.5.6).
    let objs = objects(doc);
    let intents = output_intents(doc, b"GTS_PDFX");
    match intents.as_slice() {
        [] => issue("ISO 32000-1 14.11.5", "There is no PDF/X output intent", None, None, false),
        [oi, rest @ ..] => {
            if !rest.is_empty() {
                issue("ISO 32000-1 14.11.5", "There is more than one PDF/X output intent", None, None, false);
            }
            if oi.get(b"OutputConditionIdentifier").and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).is_none_or(|s| s.trim().is_empty())
            {
                issue("ISO 32000-1 14.11.5", "The PDF/X output intent has no output condition identifier", None, None, false);
            }
            match oi.get(b"DestOutputProfile").map(|p| doc.resolve(p)) {
                Some(p) if matches!(&*p, Object::Stream(_)) => {
                    // ICC.1 header: the profile/device class is bytes 12–15.
                    let class = match &*p {
                        Object::Stream(s) => s.decoded().ok().and_then(|b| b.get(12..16).map(<[u8]>::to_vec)),
                        _ => None,
                    };
                    if class.as_deref() != Some(b"prtr") {
                        issue("ISO 32000-1 14.11.5", "The PDF/X output intent's profile isn't an output (printer) profile", None, None, false);
                    }
                    let used = uses_device_colour(doc, &objs);
                    let n = profile_components(doc, oi);
                    if used.rgb && !used.default_rgb && n != Some(3) {
                        issue("ISO 32000-1 8.6.5.6", "DeviceRGB is used, but the PDF/X output intent isn't an RGB profile", None, None, false);
                    }
                    if used.cmyk && !used.default_cmyk && n != Some(4) {
                        issue("ISO 32000-1 8.6.5.6", "DeviceCMYK is used, but the PDF/X output intent isn't a CMYK profile", None, None, false);
                    }
                }
                _ => issue("ISO 32000-1 14.11.5", "The PDF/X output intent has no destination profile", None, None, false),
            }
        }
    }
    // Pages: their boxes (14.11.2) and what prints in the bleed (12.5.3).
    let pages = prckopis_model::pages(doc);
    for (i, p) in pages.iter().enumerate() {
        let b = Boxes::of(doc, &p.dict);
        match (b.trim, b.art) {
            (None, None) => issue("ISO 32000-1 14.11.2", "The page has neither a TrimBox nor an ArtBox", None, Some(i), true),
            (Some(_), Some(_)) => issue("ISO 32000-1 14.11.2", "The page has both a TrimBox and an ArtBox", None, Some(i), true),
            _ => {}
        }
        if [b.bleed, b.trim, b.art].into_iter().flatten().any(|r| !inside(r, b.media)) {
            issue("ISO 32000-1 14.11.2", "A page box extends beyond the MediaBox", None, Some(i), true);
        }
        if let (Some(t), Some(bl)) = (b.trim, b.bleed)
            && !inside(t, bl)
        {
            issue("ISO 32000-1 14.11.2", "The TrimBox extends beyond the BleedBox", None, Some(i), false);
        }
        let bleed = b.bleed_area();
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for a in &annots {
            let Some(d) = doc.resolve(a).as_dict().cloned() else { continue };
            let sub = d.name(b"Subtype").unwrap_or_default();
            if matches!(sub, b"TrapNet" | b"PrinterMark" | b"Popup") {
                continue;
            }
            // Print flag set, Hidden clear (Table 165).
            let f = d.int(b"F").unwrap_or(0);
            if f & 4 == 0 || f & 2 != 0 {
                continue;
            }
            if rect(doc, d.get(b"Rect")).and_then(|r| intersection(r, bleed)).is_some() {
                issue(
                    "ISO 32000-1 12.5.3",
                    "A {} annotation prints inside the bleed area",
                    Some(String::from_utf8_lossy(sub).into_owned()),
                    Some(i),
                    false,
                );
            }
        }
    }
    // Graphics states: no transfer functions (8.4.5, Table 58).
    if graphics_states(doc, &objs)
        .iter()
        .any(|g| g.get(b"TR").is_some() || g.get(b"TR2").is_some_and(|t| doc.resolve(t).as_name() != Some(b"Default")))
    {
        issue("ISO 32000-1 8.4.5", "A graphics state sets a transfer function", None, None, false);
    }
    // Optional content configurations (8.11.4.3).
    for (_, c) in oc_configs(doc) {
        if c.get(b"Name").and_then(|n| doc.resolve(n).as_string().map(|s| s.to_text())).is_none_or(|n| n.trim().is_empty()) {
            issue("ISO 32000-1 8.11.4.3", "An optional content configuration has no name", None, None, true);
        }
        if c.get(b"AS").is_some() {
            issue("ISO 32000-1 8.11.4.3", "An optional content configuration has an AS entry", None, None, true);
        }
    }
    // Fonts and streams.
    for (_, o) in &objs {
        let Some(d) = dict_of(o) else { continue };
        let ty = d.name(b"Type");
        let sub = d.name(b"Subtype");
        if ty == Some(b"Font") || d.get(b"BaseFont").is_some() && sub.is_some() {
            check_font(doc, d, &mut |_clause, pattern, value, page, fixable| issue("ISO 32000-1 9.9", pattern, value, page, fixable));
        }
        if let Object::Stream(s) = &**o {
            if s.dict.get(b"F").is_some() || s.dict.get(b"FFilter").is_some() {
                issue("ISO 32000-1 7.3.8.2", "A stream refers to an external file", None, None, false);
            }
            if sub == Some(b"PS") {
                issue("ISO 32000-1 8.8.2", "PostScript XObjects aren't allowed", None, None, false);
            }
            if (sub == Some(b"Image") && s.dict.get(b"Alternates").is_some()) || s.dict.get(b"OPI").is_some() {
                issue("ISO 32000-1 8.9.5.4", "An image has alternates or OPI information", None, None, true);
            }
        }
    }
    // One message per kind of problem and page is enough for repeated ones.
    let mut seen = HashSet::new();
    out.retain(|i| seen.insert((i.clause, i.message.clone(), i.page)));
    out
}

/// Change the document information dictionary, adding one when there is none (or it is broken).
fn update_info(doc: &mut Document, f: impl FnOnce(&mut Dict)) -> Result<(), CosError> {
    match doc.trailer().get(b"Info").cloned() {
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => doc.update_dict(r, f),
        other => {
            let mut d = match other {
                Some(Object::Dict(d)) => d,
                _ => Dict::new(),
            };
            f(&mut d);
            let r = doc.add(Object::Dict(d));
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
            Ok(())
        }
    }
}

/// A file identifier made from the file's bytes (FNV-1a over a sample), two equal 16-byte strings.
fn new_id(doc: &Document) -> Object {
    let bytes = doc.bytes();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes.iter().step_by((bytes.len() / 4096).max(1)).chain(bytes.iter().rev().take(64)) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let mut id = Vec::with_capacity(16);
    id.extend_from_slice(&h.to_be_bytes());
    id.extend_from_slice(&h.rotate_left(29).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_be_bytes());
    let s = Object::String(PdfString { bytes: id, hex: true });
    Object::Array(vec![s.clone(), s])
}

/// Fix what can be fixed for PDF/X-4 without changing what prints, and say what was done (in
/// English, for scripts). `now` is a PDF date for dates the document lacks; without it they stay
/// missing. Check again with [`verify`] for what remains.
pub fn fix(doc: &mut Document, now: Option<&str>) -> Result<Vec<String>, CosError> {
    let mut fixed = Vec::new();
    if doc.trailer().get(b"Encrypt").is_some() {
        doc.remove_encryption();
        fixed.push("Removed encryption".to_owned());
    }
    if !has_id(doc) {
        let id = new_id(doc);
        doc.trailer_mut().set(b"ID".to_vec(), id);
        fixed.push("Gave the file an identifier".to_owned());
    }
    let root = doc.root().ok_or(CosError::Syntax { offset: 0, detail: "no catalog".into() })?;
    // The document information: identification, trapping and dates.
    let old = metadata(doc);
    let was_trapped = trapped(doc, old.as_deref());
    let creation = info(doc, "CreationDate").filter(|d| !d.trim().is_empty());
    let modified = info(doc, "ModDate").filter(|d| !d.trim().is_empty());
    let has_xmp = |key: &str| old.as_deref().and_then(|x| xmp::value(x, key)).is_some_and(|v| !v.trim().is_empty());
    let set_creation = creation.is_none() && !has_xmp("xmp:CreateDate") && now.is_some();
    let set_modified = modified.is_none() && !has_xmp("xmp:ModifyDate") && now.is_some();
    let version_in_info = info(doc, "GTS_PDFXVersion");
    update_info(doc, |d| {
        if version_in_info.as_deref() != Some(VERSION) {
            d.set(b"GTS_PDFXVersion".to_vec(), PdfString::text(VERSION));
        }
        if was_trapped.is_none() {
            d.set(b"Trapped".to_vec(), Object::name("False"));
        }
        if let Some(now) = now {
            if set_creation {
                d.set(b"CreationDate".to_vec(), PdfString::literal(now.as_bytes().to_vec()));
            }
            if set_modified {
                d.set(b"ModDate".to_vec(), PdfString::literal(now.as_bytes().to_vec()));
            }
        }
    })?;
    if was_trapped.is_none() {
        fixed.push("Set Trapped to False".to_owned());
    }
    if set_creation || set_modified {
        fixed.push("Added the missing dates".to_owned());
    }
    // XMP: a new packet only when the old one doesn't already say all of it, so a packet that
    // does keeps everything else it holds.
    let mut ids = xmp::Ids::declared_in(old.as_deref());
    let trapped_now = trapped(doc, None).or(was_trapped);
    let identified = ids.pdfx.as_deref().is_some_and(|v| v.starts_with(VERSION));
    let trapped_differs = ids.trapped.is_some() && ids.trapped != trapped_now;
    if old.is_none() || !identified || trapped_differs || set_creation || set_modified {
        if !identified {
            ids.pdfx = Some(VERSION.to_owned());
        }
        ids.trapped = trapped_now;
        let x = xmp::packet(doc, &ids, old.as_deref());
        let mut md = Dict::new();
        md.set(b"Type".to_vec(), Object::name("Metadata"));
        md.set(b"Subtype".to_vec(), Object::name("XML"));
        // Metadata stays uncompressed so other tools can find it.
        let m = doc.add(Object::Stream(Stream::from_raw(md, x.into_bytes())));
        doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(m)))?;
        fixed.push(format!("Identified the file as {VERSION} in its XMP metadata"));
    }
    // Page boxes.
    let mut boxes = 0usize;
    for p in prckopis_model::pages(doc) {
        let b = Boxes::of(doc, &p.dict);
        let mut changes: Vec<(&[u8], Option<[f64; 4]>)> = Vec::new();
        let (mut trim, mut art) = (b.trim, b.art);
        if trim.is_none() && art.is_none() {
            trim = Some(b.crop);
            changes.push((b"TrimBox", trim));
        } else if trim.is_some() && art.is_some() {
            art = None;
            changes.push((b"ArtBox", None));
        }
        for (key, r) in [(&b"BleedBox"[..], b.bleed), (b"TrimBox", trim), (b"ArtBox", art)] {
            if let Some(r) = r
                && !inside(r, b.media)
            {
                // A box wholly outside the media box has nothing left: the crop box stands in.
                changes.retain(|(k, _)| *k != key);
                changes.push((key, Some(intersection(r, b.media).unwrap_or(b.crop))));
            }
        }
        if changes.is_empty() {
            continue;
        }
        boxes += 1;
        doc.update_dict(p.obj, |d| {
            for (key, r) in changes {
                match r {
                    Some(r) => d.set(key.to_vec(), rect_object(r)),
                    None => {
                        d.remove(key);
                    }
                }
            }
        })?;
    }
    if boxes > 0 {
        fixed.push(format!("{boxes}: set the pages' trim and other boxes"));
    }
    // Image alternates and OPI.
    let mut images = 0usize;
    for (r, o) in objects(doc) {
        let Object::Stream(s) = &*o else { continue };
        if s.dict.get(b"OPI").is_none() && !(s.dict.name(b"Subtype") == Some(b"Image") && s.dict.get(b"Alternates").is_some()) {
            continue;
        }
        let mut ns = s.clone();
        ns.dict.remove(b"OPI");
        if ns.dict.name(b"Subtype") == Some(b"Image") {
            ns.dict.remove(b"Alternates");
        }
        doc.set(r, Object::Stream(ns));
        images += 1;
    }
    if images > 0 {
        fixed.push(format!("{images}: removed image alternates and OPI"));
    }
    // Optional content configurations.
    let configs = oc_configs(doc);
    if !configs.is_empty() {
        let mut changed = 0usize;
        let mut inline = Vec::new();
        for (n, (r, c)) in configs.into_iter().enumerate() {
            let mut nc = c.clone();
            if nc.get(b"Name").and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).is_none_or(|v| v.trim().is_empty()) {
                let name = if n == 0 { "Default".to_owned() } else { format!("Configuration {n}") };
                nc.set(b"Name".to_vec(), PdfString::text(&name));
            }
            nc.remove(b"AS");
            if nc == c {
                continue;
            }
            changed += 1;
            match r {
                Some(r) => doc.set(r, Object::Dict(nc)),
                None => inline.push((n, nc)),
            }
        }
        if !inline.is_empty() {
            // Configurations written inside OCProperties: put the fixed ones back in place.
            let cat = catalog(doc);
            let props_obj = cat.get(b"OCProperties").cloned();
            let mut props = props_obj.as_ref().map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()).unwrap_or_default();
            for (n, nc) in inline {
                if n == 0 && props.get(b"D").is_some_and(|d| d.as_ref().is_none()) {
                    props.set(b"D".to_vec(), Object::Dict(nc));
                } else if let Some(Object::Array(mut list)) = props.get(b"Configs").cloned() {
                    // `n` counts /D first; a missing /D shifts nothing because /D is taken first.
                    let at = if props.get(b"D").is_some() { n.saturating_sub(1) } else { n };
                    if let Some(slot) = list.get_mut(at)
                        && slot.as_ref().is_none()
                    {
                        *slot = Object::Dict(nc);
                    }
                    props.set(b"Configs".to_vec(), Object::Array(list));
                }
            }
            match props_obj {
                Some(Object::Ref(r)) => doc.set(r, Object::Dict(props)),
                _ => doc.update_dict(root, |c| c.set(b"OCProperties".to_vec(), Object::Dict(props)))?,
            }
        }
        if changed > 0 {
            fixed.push(format!("{changed}: named optional content configurations and removed AS"));
        }
    }
    Ok(fixed)
}
