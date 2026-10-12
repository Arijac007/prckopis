//! Bates numbering across several files (Acrobat's Bates Numbering ▸ Add with a file list): one
//! number sequence that carries on from each file to the next.

use std::sync::Arc;

use crate::{Edit, HeaderFooter, Session};

/// The most files one run numbers.
pub const MAX_FILES: usize = 1000;

/// The Bates start number in a header or footer template, if it has one.
pub fn start_of(template: &str) -> Option<u64> {
    pdfcraft_edit::bates_start(template)
}

/// One numbered file.
#[derive(Clone, Debug)]
pub struct NumberedFile {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// The first and last Bates numbers on it.
    pub first: u64,
    pub last: u64,
}

/// The result of a run: one entry per input file (a file that fails keeps its error and doesn't
/// use up numbers), and the number the next file would get.
#[derive(Debug, Default)]
pub struct Run {
    pub files: Vec<Result<NumberedFile, String>>,
    pub next: u64,
}

/// Number every page of `files`, in order, with `settings` (a header or footer with a
/// `<<Bates Number…>>` token), counting from `start`.
pub fn number_files(files: Vec<(String, Arc<Vec<u8>>)>, settings: &HeaderFooter, start: u64) -> Run {
    let mut run = Run { files: Vec::new(), next: start };
    if !settings.text.iter().any(|t| pdfcraft_edit::has_bates(t)) {
        run.files = files.into_iter().map(|(name, _)| Err(format!("{name}: the header and footer have no Bates number"))).collect();
        return run;
    }
    if files.len() > MAX_FILES {
        run.files = vec![Err(format!("number at most {MAX_FILES} files at a time"))];
        return run;
    }
    for (name, bytes) in files {
        let r = number_one(&name, bytes, settings, run.next);
        if let Ok(f) = &r {
            run.next = f.last.saturating_add(1);
        }
        run.files.push(r.map_err(|e| format!("{name}: {e}")));
    }
    run
}

fn number_one(name: &str, bytes: Arc<Vec<u8>>, settings: &HeaderFooter, start: u64) -> Result<NumberedFile, String> {
    let mut s = Session::new();
    let id = s.open(name, None, bytes, None).map_err(|e| e.to_string())?;
    let doc = s.get(id).ok_or("the document was lost")?;
    if let Some(why) = doc.read_only_reason.clone() {
        return Err(why);
    }
    let pages = doc.info.pages.len();
    if pages == 0 {
        return Err("it has no pages".into());
    }
    let mut hf = settings.clone();
    for t in &mut hf.text {
        *t = pdfcraft_edit::with_bates_start(t, start);
    }
    s.apply(id, Edit::AddHeaderFooter { pages: (0..pages).collect(), settings: hf, replace: false }).map_err(|e| e.to_string())?;
    let d = s.get(id).ok_or("the document was lost")?;
    // A signed file keeps its signatures (incremental); otherwise a clean full rewrite.
    let bytes = if d.is_signed() { s.save_bytes(id) } else { s.save_full_bytes(id) }.map_err(|e| e.to_string())?;
    let last = start.checked_add(pages as u64 - 1).ok_or("the Bates numbers are too large")?;
    Ok(NumberedFile { name: name.to_string(), bytes, first: start, last })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(pages: usize) -> Arc<Vec<u8>> {
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
        let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" "))];
        for _ in 0..pages {
            objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>".into());
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        Arc::new(out)
    }

    fn bates(prefix: &str) -> HeaderFooter {
        let mut hf = HeaderFooter::default();
        hf.text[5] = format!("<<Bates Number#6#1#{prefix}#>>");
        hf
    }

    #[test]
    fn numbers_carry_on_across_files_and_failures_use_none() {
        let files = vec![("a.pdf".to_string(), pdf(2)), ("broken.pdf".to_string(), Arc::new(b"not a pdf".to_vec())), ("b.pdf".to_string(), pdf(3))];
        let run = number_files(files, &bates("ACME-"), 100);
        assert_eq!(run.files.len(), 3);
        let a = run.files[0].as_ref().unwrap();
        assert_eq!((a.first, a.last), (100, 101));
        assert!(run.files[1].as_ref().unwrap_err().starts_with("broken.pdf: "));
        let b = run.files[2].as_ref().unwrap();
        assert_eq!((b.first, b.last), (102, 104));
        assert_eq!(run.next, 105);
        // The numbers are on the pages, as Bates marks Remove can find.
        let mut s = Session::new();
        let id = s.open("b.pdf", None, b.bytes.clone(), None).unwrap();
        let text: String = s.get(id).unwrap().text_lines(0).into_iter().map(|l| l.text).collect();
        assert!(text.contains("ACME-000102"), "{text}");
        assert!(s.apply(id, Edit::RemoveBates).is_ok());
    }

    #[test]
    fn a_header_without_a_bates_number_is_refused() {
        let mut hf = HeaderFooter::default();
        hf.text[1] = "Confidential".into();
        let run = number_files(vec![("a.pdf".into(), pdf(1))], &hf, 1);
        assert!(run.files[0].is_err());
        assert_eq!(run.next, 1);
    }
}
