//! XMP metadata for PDF/A: the identification schema and the document information, kept
//! consistent (ISO 19005-2 §6.6).

use prckopis_cos::Document;

/// The value of a simple XMP property, written either as an attribute (`pdfaid:part="2"`) or
/// as an element (`<pdfaid:part>2</pdfaid:part>`, or inside an `rdf:Alt`/`rdf:Seq`).
pub fn value(xmp: &str, key: &str) -> Option<String> {
    let attr = format!("{key}=\"");
    if let Some(i) = xmp.find(&attr) {
        let rest = &xmp[i + attr.len()..];
        return rest.find('"').map(|e| unescape(&rest[..e]));
    }
    let attr1 = format!("{key}='");
    if let Some(i) = xmp.find(&attr1) {
        let rest = &xmp[i + attr1.len()..];
        return rest.find('\'').map(|e| unescape(&rest[..e]));
    }
    let open = format!("<{key}>");
    let close = format!("</{key}>");
    let start = xmp.find(&open)? + open.len();
    let end = start + xmp[start..].find(&close)?;
    let inner = &xmp[start..end];
    // rdf:Alt / rdf:Seq: the first rdf:li.
    if let Some(li) = inner.find("<rdf:li") {
        let s = li + inner[li..].find('>')? + 1;
        let e = s + inner[s..].find("</rdf:li>")?;
        return Some(unescape(inner[s..e].trim()));
    }
    Some(unescape(inner.trim()))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`) as an XMP (ISO 8601) date.
pub fn iso_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, def: &str| digits.get(a..b).unwrap_or(def).to_string();
    let (y, mo, da, h, mi, s) = (part(0, 4, "0000"), part(4, 6, "01"), part(6, 8, "01"), part(8, 10, "00"), part(10, 12, "00"), part(12, 14, "00"));
    let rest = &d[digits.len()..];
    let tz = match rest.chars().next() {
        Some('Z') | None => "Z".to_string(),
        Some(sign @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            format!("{sign}{}:{}", t.get(0..2).unwrap_or("00"), t.get(2..4).unwrap_or("00"))
        }
        _ => "Z".to_string(),
    };
    Some(format!("{y}-{mo}-{da}T{h}:{mi}:{s}{tz}"))
}

fn info(doc: &Document, key: &str) -> Option<String> {
    let i = doc.trailer().get(b"Info").map(|i| doc.resolve(i))?;
    i.as_dict()?.get(key.as_bytes()).and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).filter(|s| !s.is_empty())
}

/// The standards an XMP packet identifies the file as conforming to, and its trapping state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ids {
    /// `pdfaid:part` and `pdfaid:conformance` (ISO 19005-2 6.6.4).
    pub pdfa: Option<(u8, String)>,
    /// `pdfxid:GTS_PDFXVersion` (ISO 15930-7), e.g. `PDF/X-4`.
    pub pdfx: Option<String>,
    /// `pdfuaid:part` (ISO 14289-1).
    pub pdfua: Option<u8>,
    /// `pdf:Trapped`: `True` or `False`.
    pub trapped: Option<&'static str>,
}

impl Ids {
    /// What the packet `old` already declares, so a new packet can keep it.
    pub fn declared_in(old: Option<&str>) -> Ids {
        let Some(x) = old else { return Ids::default() };
        let pdfa = value(x, "pdfaid:part")
            .and_then(|p| p.trim().parse().ok())
            .map(|p| (p, value(x, "pdfaid:conformance").unwrap_or_default().to_uppercase()));
        let trapped = match value(x, "pdf:Trapped").as_deref().map(str::trim) {
            Some("True") => Some("True"),
            Some("False") => Some("False"),
            _ => None,
        };
        Ids {
            pdfa,
            pdfx: value(x, "pdfxid:GTS_PDFXVersion").map(|v| v.trim().to_owned()),
            pdfua: value(x, "pdfuaid:part").and_then(|p| p.trim().parse().ok()),
            trapped,
        }
    }
}

/// The XMP packet for `doc` declaring `ids`, from its document information, or — for what the
/// document information doesn't say — from the packet `old` it replaces.
pub fn packet(doc: &Document, ids: &Ids, old: Option<&str>) -> String {
    let from_old = |key: &str| old.and_then(|x| value(x, key)).filter(|v| !v.trim().is_empty());
    let mut props = String::new();
    if let Some(t) = info(doc, "Title").or_else(|| from_old("dc:title")) {
        props.push_str(&format!("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n", esc(&t)));
    }
    if let Some(a) = info(doc, "Author").or_else(|| from_old("dc:creator")) {
        props.push_str(&format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", esc(&a)));
    }
    if let Some(s) = info(doc, "Subject").or_else(|| from_old("dc:description")) {
        props.push_str(&format!("   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n", esc(&s)));
    }
    if let Some(k) = info(doc, "Keywords").or_else(|| from_old("pdf:Keywords")) {
        props.push_str(&format!("   <pdf:Keywords>{}</pdf:Keywords>\n", esc(&k)));
    }
    if let Some(p) = info(doc, "Producer").or_else(|| from_old("pdf:Producer")) {
        props.push_str(&format!("   <pdf:Producer>{}</pdf:Producer>\n", esc(&p)));
    }
    if let Some(c) = info(doc, "Creator").or_else(|| from_old("xmp:CreatorTool")) {
        props.push_str(&format!("   <xmp:CreatorTool>{}</xmp:CreatorTool>\n", esc(&c)));
    }
    if let Some(d) = info(doc, "CreationDate").and_then(|d| iso_date(&d)).or_else(|| from_old("xmp:CreateDate")) {
        props.push_str(&format!("   <xmp:CreateDate>{}</xmp:CreateDate>\n", esc(&d)));
    }
    if let Some(d) = info(doc, "ModDate").and_then(|d| iso_date(&d)).or_else(|| from_old("xmp:ModifyDate")) {
        props.push_str(&format!("   <xmp:ModifyDate>{}</xmp:ModifyDate>\n", esc(&d)));
    }
    if let Some(t) = ids.trapped {
        props.push_str(&format!("   <pdf:Trapped>{t}</pdf:Trapped>\n"));
    }
    let mut id_props = String::new();
    let mut namespaces = String::new();
    if let Some((part, conformance)) = &ids.pdfa {
        namespaces.push_str("\n    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"");
        id_props.push_str(&format!("   <pdfaid:part>{part}</pdfaid:part>\n   <pdfaid:conformance>{}</pdfaid:conformance>\n", esc(conformance)));
    }
    if let Some(version) = &ids.pdfx {
        namespaces.push_str("\n    xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\"");
        id_props.push_str(&format!("   <pdfxid:GTS_PDFXVersion>{}</pdfxid:GTS_PDFXVersion>\n", esc(version)));
    }
    if let Some(part) = ids.pdfua {
        namespaces.push_str("\n    xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"");
        id_props.push_str(&format!("   <pdfuaid:part>{part}</pdfuaid:part>\n"));
    }
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"{namespaces}>\n\
{id_props}{props}  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_attribute_and_element_forms() {
        let a = "<rdf:Description pdfaid:part=\"2\" pdfaid:conformance='B'/>";
        assert_eq!(value(a, "pdfaid:part").as_deref(), Some("2"));
        assert_eq!(value(a, "pdfaid:conformance").as_deref(), Some("B"));
        let e = "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">A &amp; B</rdf:li></rdf:Alt></dc:title><pdfaid:part>3</pdfaid:part>";
        assert_eq!(value(e, "dc:title").as_deref(), Some("A & B"));
        assert_eq!(value(e, "pdfaid:part").as_deref(), Some("3"));
        assert_eq!(value(e, "pdfuaid:part"), None);
    }

    #[test]
    fn pdf_dates_become_iso_dates() {
        assert_eq!(iso_date("D:20240105093000+01'00'").as_deref(), Some("2024-01-05T09:30:00+01:00"));
        assert_eq!(iso_date("D:20240105").as_deref(), Some("2024-01-05T00:00:00Z"));
        assert_eq!(iso_date("D:20240105120000Z").as_deref(), Some("2024-01-05T12:00:00Z"));
        assert_eq!(iso_date("junk"), None);
    }
}
