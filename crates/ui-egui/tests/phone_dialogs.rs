//! Every dialog fits a phone (a Samsung Galaxy S22: 384 × 854 points, the phone layout): the card
//! lies within the screen, nothing in it is wider than the screen (no sideways panning), and its
//! buttons (OK / Apply / Cancel / Close …) are on the screen, where a finger can reach them.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use prckopis_ui_egui::{Dialog, PrckopisApp, PropsTab};

const FORM: &[u8] = include_bytes!("data/form.pdf");
const PHONE: egui::Vec2 = egui::vec2(384.0, 854.0);

fn phone(setup: impl FnOnce(&mut PrckopisApp) + 'static) -> Harness<'static, PrckopisApp> {
    let mut setup = Some(setup);
    let mut h = Harness::builder().with_size(PHONE).build_eframe(move |_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, FORM.to_vec()).unwrap();
        app.set_compact(true);
        if let Some(f) = setup.take() {
            f(&mut app);
        }
        app
    });
    // Grids and wrapped rows settle over a few frames.
    h.run_steps(8);
    h
}

/// The labels a dialog's buttons may have; at least one must be on the screen.
const BUTTONS: &[&str] = &[
    "OK",
    "Apply",
    "Cancel",
    "Close",
    "Export",
    "Print",
    "Recognize text",
    "Compare",
    "Start Checking",
    "Continue",
    "Save & Close",
    "Done",
    "Remove",
    "Split",
    "Revert",
    "Create PDF Comment Summary",
    "Sanitize",
    "Mark all",
    "Extract",
    "Start",
    "Save as PDF/A",
    "Run",
    "Save",
    "Recover",
];

/// What doesn't fit, if anything.
fn misfit(h: &Harness<'_, PrckopisApp>, id: &str, what: &str) -> Option<String> {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, PHONE);
    let Some(card) = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(id).with("card"))) else {
        return Some(format!("{what}: no dialog card"));
    };
    if card.left() < -0.5 || card.right() > screen.right() + 0.5 {
        return Some(format!("{what}: card {card:?} is wider than the phone"));
    }
    let overflow: f32 = h.ctx.data(|d| d.get_temp(egui::Id::new(id).with("overflow"))).unwrap_or(0.0);
    if overflow > 1.0 {
        return Some(format!("{what}: contents are {overflow} points wider than the screen"));
    }
    let mut found = 0;
    for label in BUTTONS {
        for node in h.query_all_by_label(label) {
            let r = node.rect();
            // Only buttons inside the dialog card (not the page's or the bars' own).
            if !card.expand(1.0).contains_rect(r) {
                continue;
            }
            if !screen.contains_rect(r) {
                return Some(format!("{what}: “{label}” at {r:?} is off the screen"));
            }
            found += 1;
        }
    }
    (found == 0).then(|| format!("{what}: no button found on the screen"))
}

#[track_caller]
fn assert_fits(h: &Harness<'_, PrckopisApp>, id: &str, what: &str) {
    if let Some(problem) = misfit(h, id, what) {
        panic!("{problem}");
    }
}

#[test]
fn every_named_dialog_fits_a_phone() {
    let mut problems = Vec::new();
    for name in [
        "properties",
        "security",
        "fonts",
        "advanced",
        "shortcuts",
        "split",
        "protect",
        "page-boxes",
        "header-footer",
        "watermark",
        "background",
        "export-image",
        "export-text",
        "export-all-images",
        "accessibility-options",
        "recognize-text",
        "js-console",
        "document-js",
        "preferences",
        "compare-files",
        "action-wizard",
        "pdfa",
        "signature",
        "optimize",
        "sign",
        "number-pages",
        "about",
    ] {
        let h = phone(move |app| app.set_option("dialog", name).unwrap());
        problems.extend(misfit(&h, "dialog", name));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn dialogs_opened_by_commands_fit_a_phone() {
    let mut problems = Vec::new();
    for command in ["print.dialog", "redact.properties", "redact.pages", "redact.search", "redact.sanitize", "page.extract", "page.rotate_dialog"] {
        let h = phone(move |app| {
            assert!(app.execute(command), "{command} runs");
        });
        problems.extend(misfit(&h, "dialog", command));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn dialogs_without_a_command_here_fit_a_phone() {
    // Opened directly: their commands need comments, a saved file or a selection first.
    let mut problems = Vec::new();
    for dialog in [
        Dialog::Revert,
        Dialog::SummarizeComments,
        Dialog::RedactApply,
        Dialog::RedactCodes,
        Dialog::Sanitize,
        Dialog::RemoveHidden,
        Dialog::Properties(PropsTab::InitialView),
        Dialog::AuditSpace,
        Dialog::Recovery,
        Dialog::CreateStamp,
    ] {
        let h = phone(move |app| app.dialog = Some(dialog));
        problems.extend(misfit(&h, "dialog", &format!("{dialog:?}")));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn field_properties_fit_a_phone() {
    let h = phone(|app| app.open_field_props("country", 0));
    assert_fits(&h, "dialog", "field properties");
}

#[test]
fn print_buttons_are_on_screen_and_cancel_closes_it() {
    let mut h = phone(|app| {
        app.execute("print.dialog");
    });
    assert_fits(&h, "dialog", "print");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None, "Cancel closes Print");
}

#[test]
fn header_and_footer_ok_is_reachable_without_panning() {
    let mut h = phone(|app| app.set_option("dialog", "header-footer").unwrap());
    assert_fits(&h, "dialog", "header and footer");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, None, "Cancel closes Header and Footer");
}

#[test]
fn a_desktop_window_keeps_the_wide_dialogs() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, FORM.to_vec()).unwrap();
        app.execute("print.dialog");
        app
    });
    h.run_steps(6);
    let card: egui::Rect = h.ctx.data(|d| d.get_temp(egui::Id::new("dialog").with("card"))).unwrap();
    assert!(card.width() >= 820.0, "Print keeps its desktop width: {card:?}");
}

#[test]
fn the_organize_bar_wraps_on_a_phone() {
    let h = phone(|app| app.set_option("organize", "on").unwrap());
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, PHONE);
    for label in ["Apply", "Rotate clockwise", "Move earlier", "Move later", "Save pages", "Larger pages"] {
        let r = h.get_by_label(label).rect();
        assert!(screen.contains_rect(r), "“{label}” at {r:?} is off the screen");
    }
}
