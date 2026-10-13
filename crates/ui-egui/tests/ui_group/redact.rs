//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use prckopis_ui_egui::{Dialog, PrckopisApp};

fn harness() -> Harness<'static, PrckopisApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

fn at(h: &Harness<'static, PrckopisApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn marks(h: &Harness<'static, PrckopisApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().redaction_marks()
}

#[test]
fn marking_applying_and_clearing() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.get_by_label("Redact a PDF");
    // Draw a box in an empty part of the page.
    let (a, b) = (at(&h, 40.0, 280.0), at(&h, 200.0, 320.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(marks(&h), 1);
    // Redact pages ▸ current page.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("Mark Page Range");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Clear all, then undo it.
    h.get_by_label("Clear all").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Redact all asks first; Apply removes the marks (and the page's fields under them).
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactApply));
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!((doc.redaction_marks(), doc.can_undo()), (0, Some("Apply redactions")));
    assert!(doc.form.is_empty(), "the whole page was redacted, fields included");
}

#[test]
fn redaction_codes_become_the_overlay_text() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.get_by_label("Redaction Tool Properties");
    h.get_by_label("Use overlay text").click();
    h.run_steps(2);
    h.state_mut().redact_prefs.overlay = "CONFIDENTIAL".into();
    h.get_by_label("Redaction code:").click();
    h.run_steps(2);
    h.get_by_label("(b)(7)(C)").click();
    h.run_steps(2);
    h.get_by_label("(b)(6)").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let prefs = &h.state().redact_prefs;
    assert_eq!(prefs.overlay_text(), "(b)(6), (b)(7)(C)", "codes in the set's order");
    let prckopis_engine::Edit::AddAnnotation(a) = prefs.mark(0, vec![prckopis_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, prckopis_engine::Shape::Redact { ref overlay, .. } if overlay == "(b)(6), (b)(7)(C)"));
    h.state_mut().redact_prefs.use_code = false;
    assert_eq!(h.state().redact_prefs.overlay_text(), "CONFIDENTIAL", "custom text is kept while codes are used");
}

#[test]
fn custom_redaction_code_sets_are_edited_exported_imported_and_kept() {
    use prckopis_ui_egui::RedactCodeSetChoice;
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(4);
    h.get_by_label("Use overlay text").click();
    h.run_steps(4);
    h.get_by_label("Edit codes…").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::RedactCodes));
    h.get_by_label("Redaction Code Editor");
    h.get_by_label("No code sets yet. Type a name and choose New set.");
    // A name the built-in sets use is refused, with a message.
    h.state_mut().redact_code_editor.set_name = "U.S. FOIA".into();
    h.get_by_label("New set").click();
    h.run_steps(4);
    h.get_by_label("That name is already used.");
    h.state_mut().redact_code_editor.set_name = "GDPR".into();
    h.get_by_label("New set").click();
    h.run_steps(4);
    for (code, description) in [("Art. 9", "Special categories"), ("Art. 10", "Criminal convictions"), ("Art. 99", "")] {
        let d = &mut h.state_mut().redact_code_editor;
        (d.code_text, d.description) = (code.into(), description.into());
        h.get_by_label("Add").click();
        // The centred dialog grows by a row and settles over a few frames.
        h.run_steps(4);
    }
    let added: Vec<String> =
        h.state().redact_prefs.code_sets.get("GDPR").map(|s| s.codes.iter().map(|c| c.code.clone()).collect()).unwrap_or_default();
    assert_eq!(added, ["Art. 9", "Art. 10", "Art. 99"], "{:?}", h.state().redact_code_editor);
    h.get_by_label("Art. 9 — Special categories");
    // Change a code, remove another, rename the set.
    h.get_by_label("Art. 10 — Criminal convictions").click();
    h.run_steps(4);
    h.state_mut().redact_code_editor.code_text = "Art. 10(1)".into();
    h.get_by_label("Update").click();
    h.run_steps(4);
    h.get_by_label("Art. 99").click();
    h.run_steps(4);
    h.get_by_label("Remove").click();
    h.run_steps(4);
    h.state_mut().redact_code_editor.set_name = "EU GDPR".into();
    h.get_by_label("Rename").click();
    h.run_steps(4);
    let set = h.state().redact_prefs.code_sets.get("EU GDPR").cloned().expect("renamed");
    let codes: Vec<&str> = set.codes.iter().map(|c| c.code.as_str()).collect();
    assert_eq!(codes, ["Art. 9", "Art. 10(1)"]);
    // Export, then import the file back: the copy gets a free name.
    let file = std::env::temp_dir().join(format!("prckopis-codes-{}.json", std::process::id()));
    h.state_mut().save_override = Some(file.to_string_lossy().into_owned());
    h.get_by_label("Export…").click();
    h.run_steps(4);
    let bytes = std::fs::read(&file).expect("exported");
    let _ = std::fs::remove_file(&file);
    h.state_mut().use_files(prckopis_ui_egui::FilePurpose::RedactCodes, vec![("codes.json".into(), bytes)]);
    let names: Vec<String> = h.state().redact_prefs.code_sets.sets().iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, ["EU GDPR", "EU GDPR (2)"]);
    assert_eq!(h.state().redact_prefs.code_sets.sets()[1].codes, set.codes, "codes and descriptions survive the round trip");
    // Untrusted files change nothing.
    h.state_mut().use_files(prckopis_ui_egui::FilePurpose::RedactCodes, vec![("bad.json".into(), b"{\"format\":1}".to_vec())]);
    h.state_mut().use_files(prckopis_ui_egui::FilePurpose::RedactCodes, vec![("huge.json".into(), vec![b' '; (1 << 20) + 1])]);
    assert_eq!(h.state().redact_prefs.code_sets.sets().len(), 2);
    h.run_steps(4);
    h.get_by_label("Done").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, Some(Dialog::RedactProps));
    // Pick the set and its codes in Redaction properties.
    h.get_by_label("Redaction code:").click();
    h.run_steps(4);
    h.get_by_value("U.S. FOIA").click();
    h.run_steps(4);
    h.get_by_label("EU GDPR").click();
    h.run_steps(4);
    assert_eq!(h.state().redact_prefs.code_set, RedactCodeSetChoice::Custom("EU GDPR".into()));
    h.get_by_label("Art. 10(1)").click();
    h.run_steps(4);
    h.get_by_label("Art. 9").click();
    h.run_steps(4);
    h.get_by_label("OK").click();
    h.run_steps(4);
    let prefs = h.state().redact_prefs.clone();
    assert_eq!(prefs.overlay_text(), "Art. 9, Art. 10(1)", "codes in the set's order");
    let prckopis_engine::Edit::AddAnnotation(a) = prefs.mark(0, vec![prckopis_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, prckopis_engine::Shape::Redact { ref overlay, .. } if overlay == "Art. 9, Art. 10(1)"));
    // The sets are kept in the preferences; unreadable saved sets are dropped.
    let saved = h.state().persist();
    let mut again = PrckopisApp::new();
    again.restore(&saved);
    assert_eq!(again.redact_prefs.code_sets, prefs.code_sets);
    let mut v: serde_json::Value = serde_json::from_str(&saved).unwrap();
    v["redaction_code_sets"] =
        serde_json::json!({ "format": "prckopis-redaction-codes", "sets": [{ "name": 5 }, { "name": "Ok", "codes": [{ "code": "" }] }] });
    let mut damaged = PrckopisApp::new();
    damaged.restore(&v.to_string());
    assert_eq!(damaged.redact_prefs.code_sets.sets().len(), 1);
    assert!(damaged.redact_prefs.code_sets.sets()[0].codes.is_empty());
}

#[test]
fn redacting_an_imported_word_list() {
    let memo = prckopis_engine::Session::new().create_from_text("memo", "Call Ada today\nAda is private\nPublic line").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.run_steps(2);
    h.get_by_label("Multiple words or phrases");
    let list = b"  Ada \n\nprivate\nAda\nabsent\n".to_vec();
    h.state_mut().use_files(prckopis_ui_egui::FilePurpose::RedactWords, vec![("names.txt".into(), list)]);
    let d = &h.state().redact_search;
    assert_eq!((d.mode, d.words.as_str()), (prckopis_ui_egui::RedactSearchMode::Words, "Ada\nprivate\nabsent"));
    h.run_steps(2);
    h.get_by_label("3 word(s) or phrase(s)");
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (3, Some(3)));
    let too_big = vec![b'a'; 2 << 20];
    h.state_mut().use_files(prckopis_ui_egui::FilePurpose::RedactWords, vec![("huge.txt".into(), too_big)]);
    assert_eq!(h.state().redact_search.words, "Ada\nprivate\nabsent", "an oversized list changes nothing");
}

#[test]
fn redaction_search_can_match_case_and_whole_words() {
    let memo = prckopis_engine::Session::new().create_from_text("memo", "Ada ada Adaline\nAda beta betamax").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.state_mut().redact_search.text = "Ada".into();
    h.run_steps(2);
    h.get_by_label("Whole words only").click();
    h.get_by_label("Case-sensitive").click();
    h.run_steps(2);
    assert!(h.state().redact_search.whole_words && h.state().redact_search.case_sensitive);
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (2, Some(2)));
}

#[test]
fn removing_hidden_information_and_sanitizing() {
    let mut h = harness();
    h.state_mut().execute("protect.remove_hidden");
    h.run_steps(2);
    h.get_by_label("Remove hidden information");
    {
        let d = &h.state().hidden_draft;
        let fields = d.found.iter().find(|f| f.0 == prckopis_engine::Hidden::FormFields).unwrap();
        assert_eq!((fields.1, fields.2), (5, true), "five fields, checked");
    }
    h.get_by_label("Remove").click();
    h.run_steps(3);
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.is_empty());
        assert_eq!(doc.can_undo(), Some("Remove hidden information"));
    }
    h.state_mut().execute("redact.sanitize");
    h.run_steps(2);
    h.get_by_label("Sanitize").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Sanitize document"));
}
