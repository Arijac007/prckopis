//! Edit ▸ Search a folder…: every PDF in a folder searched in the background; a result opens the
//! file at its page with the phrase found.

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

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

fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-folder-search-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("contract.pdf"), pdf(&["Introduction", "Payment is due by invoice"])).unwrap();
    std::fs::write(dir.join("other.pdf"), pdf(&["Nothing here"])).unwrap();
    dir
}

#[test]
fn searching_a_folder_lists_matches_and_opens_one_at_its_page() {
    let dir = folder();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.search_folder"));
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![dir.to_string_lossy().into_owned()]);
    h.get_by_label("Choose Folder…").click();
    h.run_steps(4);
    assert_eq!(h.state().folder_search.folder.as_ref(), Some(&dir));
    h.state_mut().folder_search.query = "invoice".into();
    h.run_steps(2);
    h.get_by_label("Search").click();
    // The search runs on another thread: wait for it.
    for _ in 0..400 {
        h.run_steps(1);
        if !h.state().folder_search.is_running() && h.state().folder_search.report.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let report = h.state().folder_search.report.clone().expect("the search finished");
    assert_eq!(report.files_found, 2);
    assert_eq!(report.hits.len(), 1);
    assert_eq!(report.hits[0].page, 1);
    h.get_by_label("1 instance in 2 of 2 files");
    h.get_by_label_contains("Payment is due by invoice").hover();
    h.run_steps(2);
    h.get_by_label_contains("Payment is due by invoice").click();
    h.run_steps(6);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the file opened");
    let view = &app.views[0];
    assert_eq!(view.current, 1, "at the matching page");
    assert_eq!(view.find.as_ref().map(|f| f.query.as_str()), Some("invoice"));
    let _ = std::fs::remove_dir_all(&dir);
}
