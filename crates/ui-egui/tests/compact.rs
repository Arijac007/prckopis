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

#[test]
fn the_theme_button_and_more_menu_are_on_the_phone_bar() {
    use prckopis_ui_egui::theme::ThemePreference;
    let mut h = phone(true);
    // The theme button cycles system, light, dark, as on the desktop's title bar.
    let start = h.state().theme_preference;
    h.get_by_label_contains("Display theme: ").click();
    h.run_steps(2);
    assert_ne!(h.state().theme_preference, start);
    for _ in 0..3 {
        if h.state().theme_preference == ThemePreference::Dark {
            break;
        }
        h.get_by_label_contains("Display theme: ").click();
        h.run_steps(2);
    }
    assert_eq!(h.state().theme_preference, ThemePreference::Dark);
}

#[test]
fn the_phone_menu_is_pages_of_large_rows() {
    use prckopis_ui_egui::phone_menu::PhoneMenu;
    let mut h = phone(true);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    assert_eq!(h.state().compact_menu, Some(PhoneMenu::Root));
    // The first page: the document, the workspaces, the way to the other pages.
    for item in ["Open…", "Save as…", "Print…", "Document properties…", "All tools", "Read", "E-Sign", "Side panels", "Preferences…"] {
        // (All tools is also the top bar's button.)
        assert!(h.query_all_by_label(item).count() >= 1, "{item} is on the first page");
    }
    // A menu opens as a page of its own; Back returns to the first page, and closes it there.
    // (The menus are below the fold at this size: open the page directly.)
    h.state_mut().compact_menu = Some(PhoneMenu::Commands("Pages"));
    h.run_steps(2);
    h.get_by_label("Organize pages");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().compact_menu, Some(PhoneMenu::Root));
    // A workspace from the menu: its tools open in the sheet and the menu closes. (The first
    // "Edit" is the workspace; the Edit menu's row comes later, below the fold.)
    h.query_all_by_label("Edit").next().expect("the Edit workspace").click();
    h.run_steps(2);
    assert_eq!(h.state().compact_menu, None);
    assert!(h.state().left_open, "a workspace opens its tools in the sheet");
    // A side panel from the menu's page of panels.
    h.state_mut().left_open = false;
    h.state_mut().compact_menu = Some(PhoneMenu::Panels);
    h.run_steps(2);
    h.get_by_label("Form fields").click();
    h.run_steps(2);
    assert_eq!((h.state().compact_menu, h.state().right), (None, Some(RightPanel::Fields)));
    // A row runs its command: Document properties opens its dialog.
    h.state_mut().choose_right_panel(None);
    h.state_mut().compact_menu = Some(PhoneMenu::Root);
    h.run_steps(2);
    h.get_by_label("Document properties…").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_some() && h.state().compact_menu.is_none());
}
