//! Search all PDFs in a folder (Acrobat's Advanced Search ▸ All PDF documents in): the text of
//! every page of every PDF under a folder, matched like Find, one hit per match with its line.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// At most this many files are searched.
pub const MAX_FILES: usize = 5000;
/// At most this many hits are kept (the search stops there and says so).
pub const MAX_HITS: usize = 5000;
/// Folders are followed at most this deep.
const MAX_DEPTH: usize = 32;
/// Files larger than this are skipped (they are listed as errors).
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// The longest line of context shown with a hit, in characters.
const CONTEXT_CHARS: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub case_sensitive: bool,
    pub whole_words: bool,
    /// Include subfolders.
    pub subfolders: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { case_sensitive: false, whole_words: false, subfolders: true }
    }
}

/// One match.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// 0-based.
    pub page: usize,
    /// The line (or lines) the match is on, shortened around it.
    pub context: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub hits: Vec<Hit>,
    /// PDFs whose text was read (wholly or until the search stopped).
    pub files_searched: usize,
    /// PDFs listed under the folder.
    pub files_found: usize,
    /// Files that couldn't be searched (password protected, damaged, too large), with the reason.
    pub errors: Vec<(PathBuf, String)>,
    /// Stopped early: more than [`MAX_HITS`] matches, more than [`MAX_FILES`] files, or cancelled.
    pub truncated: bool,
    pub cancelled: bool,
}

/// The PDFs under `dir`, sorted by path. Hidden files and folders (names starting with `.`) are
/// skipped and symbolic links are not followed, so a link loop can't run forever.
pub fn list_pdfs(dir: &Path, subfolders: bool) -> Result<(Vec<PathBuf>, bool), String> {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut first = true;
    while let Some((d, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) if first => return Err(format!("{}: {e}", d.display())),
            // A subfolder that can't be read is left out, not fatal.
            Err(_) => continue,
        };
        first = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if subfolders && depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                if out.len() >= MAX_FILES {
                    truncated = true;
                    break;
                }
                out.push(path);
            }
        }
        if truncated {
            break;
        }
    }
    out.sort();
    Ok((out, truncated))
}

/// Search every PDF under `dir` for `query`. `progress(done, total, path)` is called before each
/// file; setting `cancel` stops after the current page.
pub fn search_folder(
    dir: &Path,
    query: &str,
    opts: Options,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize, &Path),
) -> Result<Report, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("type what to search for".into());
    }
    let (files, too_many) = list_pdfs(dir, opts.subfolders)?;
    let mut report = Report { files_found: files.len(), truncated: too_many, ..Report::default() };
    for (i, path) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            report.truncated = true;
            break;
        }
        progress(i, files.len(), path);
        match search_file(path, query, opts, cancel, &mut report.hits) {
            Ok(()) => report.files_searched += 1,
            Err(e) => report.errors.push((path.clone(), e)),
        }
        if report.hits.len() >= MAX_HITS {
            report.hits.truncate(MAX_HITS);
            report.truncated = true;
            break;
        }
    }
    Ok(report)
}

fn search_file(path: &Path, query: &str, opts: Options, cancel: &AtomicBool, hits: &mut Vec<Hit>) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE_BYTES {
        return Err("the file is too large to search".into());
    }
    let bytes = Arc::new(std::fs::read(path).map_err(|e| e.to_string())?);
    let pages = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| e.to_string())?.pages.len();
    let mut r = PageRenderer::new(bytes, RenderConfig::default());
    for page in 0..pages {
        if cancel.load(Ordering::Relaxed) || hits.len() >= MAX_HITS {
            return Ok(());
        }
        let out = r.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        if let Some(e) = out.error {
            // A password-protected file fails on its first page; one bad page doesn't end the file.
            if page == 0 {
                return Err(e);
            }
            continue;
        }
        let Some(text) = out.text else { continue };
        let room = MAX_HITS.saturating_sub(hits.len());
        for range in text.find_opts_limited(query, opts.case_sensitive, opts.whole_words, room) {
            hits.push(Hit { path: path.to_path_buf(), page, context: context(&text, range) });
        }
    }
    Ok(())
}

/// The whole line(s) a match is on, shortened to about [`CONTEXT_CHARS`] around the match.
fn context(text: &pdfcraft_render::PageText, range: std::ops::Range<usize>) -> String {
    let n = text.glyphs.len();
    let line = |i: usize| text.line_of.get(i).copied();
    let (Some(first_line), Some(last_line)) = (line(range.start), line(range.end.saturating_sub(1).max(range.start))) else {
        return String::new();
    };
    let mut start = range.start.min(n);
    while start > 0 && line(start - 1) == Some(first_line) {
        start -= 1;
    }
    let mut end = range.end.min(n);
    while end < n && line(end) == Some(last_line) {
        end += 1;
    }
    let before: String = text.text_of(start..range.start.min(n));
    let matched = text.text_of(range.start.min(n)..range.end.min(n));
    let after: String = text.text_of(range.end.min(n)..end);
    let flat = |s: String| s.replace('\n', " ");
    let (before, matched, after) = (flat(before), flat(matched), flat(after));
    let room = CONTEXT_CHARS.saturating_sub(matched.chars().count()) / 2;
    let before_count = before.chars().count();
    let before = if before_count > room { format!("…{}", before.chars().skip(before_count - room).collect::<String>()) } else { before };
    let after = if after.chars().count() > room { format!("{}…", after.chars().take(room).collect::<String>()) } else { after };
    let sep = |s: &str, at_end: bool| if s.is_empty() || (at_end && s.ends_with(' ')) || (!at_end && s.starts_with(' ')) { "" } else { " " };
    // `text_of` drops the space at a range boundary: put one back between the parts.
    let joined_before = if before.is_empty() { String::new() } else { format!("{before}{}", sep(&before, true)) };
    let joined_after = if after.is_empty() { String::new() } else { format!("{}{after}", sep(&after, false)) };
    format!("{joined_before}{matched}{joined_after}").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF whose pages each show one line of Helvetica text.
    fn pdf(lines: &[&str]) -> Vec<u8> {
        let n = lines.len();
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!("<< /Type /Pages /Kids [{}] /Count {n} >>", (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        ];
        for (i, l) in lines.iter().enumerate() {
            let content = format!("BT /F1 12 Tf 50 700 Td ({l}) Tj ET");
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * i
            ));
            objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
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
        out
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-folder-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("contract.pdf"), pdf(&["Payment terms: net 30 days", "Invoice total"])).unwrap();
        std::fs::write(dir.join("sub").join("memo.PDF"), pdf(&["The invoice is attached"])).unwrap();
        std::fs::write(dir.join(".hidden").join("x.pdf"), pdf(&["invoice"])).unwrap();
        std::fs::write(dir.join("notes.txt"), b"invoice").unwrap();
        std::fs::write(dir.join("broken.pdf"), b"not a pdf").unwrap();
        dir
    }

    #[test]
    fn finds_matches_in_every_pdf_under_the_folder() {
        let dir = folder("all");
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let r = search_folder(&dir, "invoice", Options::default(), &cancel, |_, _, p| seen.push(p.to_path_buf())).unwrap();
        assert_eq!(r.files_found, 3, "PDFs only, hidden folders skipped");
        assert_eq!(seen.len(), 3);
        let found: Vec<(String, usize)> = r.hits.iter().map(|h| (h.path.file_name().unwrap().to_string_lossy().into_owned(), h.page)).collect();
        assert_eq!(found, [("contract.pdf".to_string(), 1), ("memo.PDF".to_string(), 0)]);
        assert!(r.hits[1].context.contains("The invoice is attached"), "{:?}", r.hits[1]);
        assert_eq!(r.errors.len(), 1, "the damaged file is reported: {:?}", r.errors);
        assert!(r.errors[0].0.ends_with("broken.pdf"));
        // Case-sensitive, and without subfolders.
        let opts = Options { case_sensitive: true, subfolders: false, ..Options::default() };
        let r = search_folder(&dir, "Invoice", opts, &cancel, |_, _, _| {}).unwrap();
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.files_found, 2);
        // Whole words.
        let opts = Options { whole_words: true, ..Options::default() };
        assert!(search_folder(&dir, "invoic", opts, &cancel, |_, _, _| {}).unwrap().hits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_stops_and_bad_input_is_refused() {
        let dir = folder("cancel");
        let cancel = AtomicBool::new(true);
        let r = search_folder(&dir, "invoice", Options::default(), &cancel, |_, _, _| {}).unwrap();
        assert!(r.cancelled && r.truncated && r.hits.is_empty());
        let go = AtomicBool::new(false);
        assert!(search_folder(&dir, "  ", Options::default(), &go, |_, _, _| {}).is_err());
        assert!(search_folder(&dir.join("missing"), "x", Options::default(), &go, |_, _, _| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
