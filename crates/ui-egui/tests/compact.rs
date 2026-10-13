//! The phone layout (`PrckopisApp::compact`, Android): one top bar, page navigation at the
//! bottom, the tools and side panels as a sheet over the page, Back (Escape) closing the sheet.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use prckopis_ui_egui::{PrckopisApp, RightPanel};

const FORM: &[u8] = include_bytes!("data/form.pdf");

fn phone(compact: bool) -> Harness<'static, PrckopisApp> {
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 860.0)).build_eframe(move |_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, FORM.to_vec()).unwrap();
        app.set_compact(compact);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn a_phone_shows_the_page_with_its_bars_and_no_side_panels() {
    let mut h = phone(true);
    assert!(!h.state().left_open && h.state().right.is_none(), "no panel covers the page");
    // Top bar and bottom bar, not the desktop's mode bar and rail.
    h.get_by_label("All tools");
    h.get_by_label("Find tools and commands");
    h.get_by_label("Next page");
    h.get_by_label("Side panels");
    assert!(h.query_by_label("E-Sign").is_none(), "no mode tabs");
    // Fit width: the page fills the screen but for a thin gutter.
    let zoom = h.state().views[0].zoom;
    h.get_by_label("Zoom in").click();
    h.run_steps(2);
    assert!(h.state().views[0].zoom > zoom);
}

#[test]
fn the_tools_and_side_panels_open_as_a_sheet_and_back_closes_them() {
    let mut h = phone(true);
    h.get_by_label("All tools").click();
    h.run_steps(2);
    assert!(h.state().left_open);
    assert!(h.query_all_by_label("Fill & Sign").count() >= 1, "All tools is listed");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().left_open, "Back closes the tools");
    h.state_mut().choose_right_panel(Some(RightPanel::Fields));
    h.run_steps(2);
    h.get_by_label("country");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().right, None, "Back closes the side panel");
}

#[test]
fn the_document_switcher_goes_home_and_back() {
    let mut h = phone(true);
    h.get_by_label("form.pdf").click();
    h.run_steps(2);
    h.get_by_label("Home").click();
    h.run_steps(2);
    assert_eq!(h.state().active, None);
    h.get_by_label("Home").click();
    h.run_steps(2);
    h.get_by_label("form.pdf").click();
    h.run_steps(2);
    assert_eq!(h.state().active, Some(0));
}

#[test]
fn a_wide_window_keeps_the_desktop_layout() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PrckopisApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, FORM.to_vec()).unwrap();
        app.set_compact(true);
        app
    });
    h.run_steps(4);
    h.get_by_label("E-Sign");
}
