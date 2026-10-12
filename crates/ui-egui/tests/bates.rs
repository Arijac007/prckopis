//! Bates numbering: numbering more files after the open document, continuing the sequence, and
//! Bates numbering ▸ Remove.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use prckopis_ui_egui::PrckopisApp;

/// A PDF with `pages` empty pages.
fn pdf(pages: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" "))];
    for _ in 0..pages {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>".into());
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

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prckopis-bates-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn page_text(bytes: Vec<u8>, page: usize) -> String {
    let mut s = prckopis_engine::Session::new();
    let id = s.open("x.pdf", None, Arc::new(bytes), None).unwrap();
    s.get(id).unwrap().text_lines(page).into_iter().map(|l| l.text).collect()
}

#[test]
fn bates_numbers_more_files_after_the_open_document_and_removes() {
    let dir = temp("more");
    let other = dir.join("exhibit.pdf");
    std::fs::write(&other, pdf(2)).unwrap();
    let out = dir.join("numbered");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("main.pdf", None, pdf(3)).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.bates"));
    h.run_steps(4);
    h.state_mut().pick_override = Some(vec![other.to_string_lossy().into_owned()]);
    h.get_by_label("Add Files…").click();
    h.run_steps(4);
    assert_eq!(h.state().marks_draft.more_files, std::slice::from_ref(&other));
    h.state_mut().pick_override = Some(vec![out.to_string_lossy().into_owned()]);
    h.get_by_label("Choose Folder…").click();
    h.run_steps(4);
    assert_eq!(h.state().marks_draft.out_dir.as_ref(), Some(&out));
    h.get_by_label("OK").click();
    h.run_steps(4);
    // The open document has 1–3; the other file carries on with 4 and 5.
    let numbered = std::fs::read(out.join("exhibit_bates.pdf")).expect("the numbered copy");
    assert!(page_text(numbered.clone(), 0).contains("000004"));
    assert!(page_text(numbered, 1).contains("000005"));
    assert_eq!(std::fs::read(&other).unwrap(), pdf(2), "the original is not changed");
    // Bates numbering ▸ Remove on the open document.
    assert!(h.state_mut().execute("edit.bates.remove"));
    h.run_steps(2);
    let _ = std::fs::remove_dir_all(&dir);
}
