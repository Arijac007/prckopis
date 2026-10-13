//! The phone layout's menu: a full page of large, finger-sized rows instead of the desktop's
//! nested drop-down menus, navigated like a phone's settings — a row opens the next page, the
//! arrow (or Back) returns to the one before. It covers what the desktop's menus, mode bar, tab
//! strip, rail and title bar offer.

use egui::{Align, Layout, Sense, Stroke, vec2};

use crate::canvas::Fit;
use crate::theme::{self, ThemePreference, Tokens};
use crate::{Dialog, Mode, PrckopisApp, PropsTab, RightPanel, icons};
use prckopis_engine::commands;

/// Which page of the phone menu is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneMenu {
    /// Everything at a glance: the document, workspaces and the way to the other pages.
    Root,
    /// One of the desktop's menus (`File`, `Edit`, `Pages`, `View`, `Help`): all its commands.
    Commands(&'static str),
    /// Recently opened files.
    Recent,
    /// The open documents (and Home): the phone's tab strip.
    Documents,
    /// The side panels (thumbnails, bookmarks, comments …).
    Panels,
    /// The display theme.
    Theme,
}

impl PhoneMenu {
    /// The page Back returns to, or `None` to close the menu.
    pub fn parent(self) -> Option<PhoneMenu> {
        match self {
            PhoneMenu::Root => None,
            _ => Some(PhoneMenu::Root),
        }
    }
}

/// Height of a menu row: comfortably more than a fingertip.
const ROW: f32 = 52.0;

/// The side panels the menu offers, with their icons.
const PANELS: [(RightPanel, &str, &str); 9] = [
    (RightPanel::Pages, "files", "Page thumbnails"),
    (RightPanel::Bookmarks, "bookmark", "Bookmarks"),
    (RightPanel::Comments, "message-square-text", "Comments"),
    (RightPanel::Fields, "text-cursor-input", "Form fields"),
    (RightPanel::Signatures, "signature", "Signatures"),
    (RightPanel::Attachments, "paperclip", "Attachments"),
    (RightPanel::Layers, "layers", "Layers"),
    (RightPanel::Tags, "tag", "Tags"),
    (RightPanel::Search, "search", "Search"),
];

/// The desktop's menus, as pages.
const MENUS: [(&str, &str, &str); 5] = [
    ("File", "file-text", "File"),
    ("Edit", "pencil", "Edit"),
    ("Pages", "files", "Pages"),
    ("View", "eye", "View"),
    ("Help", "circle-help", "Help"),
];

/// What a row leads to.
enum Go {
    Page(PhoneMenu),
    Close,
}

/// One row: an icon, the label, and on the right a check mark (`selected`) or an arrow
/// (`more`: it opens a page). Disabled rows are dimmed and don't respond.
fn row(ui: &mut egui::Ui, icon: &str, label: &str, enabled: bool, selected: bool, more: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), if enabled { Sense::click() } else { Sense::hover() });
    // Drawn by hand: say what it is, for screen readers and the control channel.
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label));
    let painter = ui.painter_at(rect);
    if enabled && (resp.hovered() || resp.is_pointer_button_down_on()) {
        painter.rect_filled(rect, 8.0, t.hover);
    }
    let ink = if enabled { t.text } else { t.text_faint };
    let icon_ink = if enabled { t.text_muted } else { t.text_faint };
    let mid = rect.center().y;
    icons::paint(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 26.0, mid), vec2(22.0, 22.0)), icon, 22.0, icon_ink);
    let right = rect.right() - 16.0;
    let label_room = (right - rect.left() - 56.0 - 28.0).max(40.0);
    let galley = ui.painter().layout(label.to_owned(), theme::regular(16.0), ink, label_room);
    painter.galley(egui::pos2(rect.left() + 52.0, mid - galley.size().y / 2.0), galley, ink);
    let mark = if selected {
        Some("check")
    } else if more {
        Some("chevron-right")
    } else {
        None
    };
    if let Some(mark) = mark {
        let tint = if selected { t.accent } else { t.text_faint };
        icons::paint(ui, egui::Rect::from_center_size(egui::pos2(right - 10.0, mid), vec2(20.0, 20.0)), mark, 20.0, tint);
    }
    painter.hline(rect.x_range().shrink(12.0), rect.bottom(), Stroke::new(0.5, t.divider));
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// A small heading between groups of rows.
fn heading(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(text.to_uppercase()).font(theme::medium(12.0)).color(t.text_faint));
    });
    ui.add_space(4.0);
}

/// The page's title bar: Back (or Close on the first page) and the title.
fn title_bar(ui: &mut egui::Ui, page: PhoneMenu, title: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut back = false;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW), Layout::left_to_right(Align::Center), |ui| {
        let (icon, tip) = if page == PhoneMenu::Root { ("x", tl!("Close")) } else { ("chevron-left", tl!("Back")) };
        if icons::button(ui, icon, 44.0, false, tip).clicked() {
            back = true;
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(title).font(theme::semibold(19.0)).color(t.text));
    });
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.divider));
    back
}

/// Draw the phone menu page `page` filling `ui`. Returns the page to show next (`None`: close).
pub(crate) fn show(app: &mut PrckopisApp, ui: &mut egui::Ui, page: PhoneMenu) -> Option<PhoneMenu> {
    let title: String = match page {
        PhoneMenu::Root => tl!("Menu").to_owned(),
        PhoneMenu::Commands(menu) => MENUS.iter().find(|(m, ..)| *m == menu).map_or(menu, |(_, _, l)| tl!(l)).to_owned(),
        PhoneMenu::Recent => {
            let label = commands::command("file.open_recent")
                .map(|s| commands::current_label(s, &app.session, app.active_ids().map(|(_, id)| id)))
                .unwrap_or_default();
            crate::i18n::menu_label("file.open_recent", &label).trim_end_matches('…').to_owned()
        }
        PhoneMenu::Documents => tl!("Open documents").to_owned(),
        PhoneMenu::Panels => tl!("Side panels").to_owned(),
        PhoneMenu::Theme => tl!("Display theme").to_owned(),
    };
    if title_bar(ui, page, &title) {
        return page.parent();
    }
    let mut go: Option<Go> = None;
    egui::ScrollArea::vertical().id_salt(("phone-menu", format!("{page:?}"))).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        match page {
            PhoneMenu::Root => root(app, ui, &mut go),
            PhoneMenu::Commands(menu) => command_page(app, ui, menu, &mut go),
            PhoneMenu::Recent => recent_page(app, ui, &mut go),
            PhoneMenu::Documents => documents_page(app, ui, &mut go),
            PhoneMenu::Panels => panels_page(app, ui, &mut go),
            PhoneMenu::Theme => theme_page(app, ui, &mut go),
        }
        ui.add_space(24.0);
    });
    match go {
        None => Some(page),
        Some(Go::Page(next)) => Some(next),
        Some(Go::Close) => None,
    }
}

/// Run a registered command from a row (and close the menu).
fn run(app: &mut PrckopisApp, ui: &mut egui::Ui, id: &str, icon: &str, go: &mut Option<Go>) {
    let Some(spec) = commands::command(id) else { return };
    let label = commands::current_label(spec, &app.session, app.active_ids().map(|(_, id)| id));
    let label = crate::i18n::menu_label(spec.id, &label);
    if row(ui, icon, &label, app.command_enabled(spec), false, false).clicked() {
        app.execute(spec.id);
        *go = Some(Go::Close);
    }
}

fn root(app: &mut PrckopisApp, ui: &mut egui::Ui, go: &mut Option<Go>) {
    let has_doc = app.active.is_some();
    heading(ui, tl!("File"));
    run(app, ui, "file.open", "folder-open", go);
    if let Some(spec) = commands::command("file.open_recent") {
        let label = crate::i18n::menu_label(spec.id, &commands::current_label(spec, &app.session, None));
        if row(ui, "clock", &label, !app.recent.is_empty(), false, true).clicked() {
            *go = Some(Go::Page(PhoneMenu::Recent));
        }
    }
    let open = app.views.len();
    let documents = format!("{} ({open})", tl!("Open documents"));
    if row(ui, "files", &documents, true, false, true).clicked() {
        *go = Some(Go::Page(PhoneMenu::Documents));
    }
    run(app, ui, "file.save", "save", go);
    run(app, ui, "file.save_as", "save", go);
    run(app, ui, "print.dialog", "printer", go);
    if row(ui, "info", tl!("Document properties…"), has_doc, false, false).clicked() {
        app.dialog = Some(Dialog::Properties(PropsTab::Description));
        *go = Some(Go::Close);
    }
    run(app, ui, "file.close", "x", go);

    heading(ui, tl!("Tools"));
    for (mode, icon, label) in [
        (Mode::AllTools, "layout-grid", "All tools"),
        (Mode::Read, "book-open", "Read"),
        (Mode::Edit, "file-pen-line", "Edit"),
        (Mode::Convert, "file-output", "Convert"),
        (Mode::Sign, "signature", "E-Sign"),
    ] {
        if row(ui, icon, tl!(label), true, app.mode == mode, false).clicked() {
            // Read mode is the page alone; the others open their tools in the sheet.
            app.switch_mode(mode, mode != Mode::Read);
            if mode == Mode::Read {
                app.left_open = false;
            }
            *go = Some(Go::Close);
        }
    }

    heading(ui, tl!("View"));
    if row(ui, "panel-right", tl!("Side panels"), has_doc, app.right.is_some(), true).clicked() {
        *go = Some(Go::Page(PhoneMenu::Panels));
    }
    if let Some(view) = app.active.and_then(|i| app.views.get_mut(i)) {
        if row(ui, "maximize", tl!("Fit to width"), true, view.fit == Fit::Width, false).clicked() {
            view.fit = Fit::Width;
            view.goto = Some((view.current, 0.0));
            *go = Some(Go::Close);
        }
        if row(ui, "maximize-2", tl!("Zoom to page level"), true, view.fit == Fit::Page, false).clicked() {
            view.fit = Fit::Page;
            view.goto = Some((view.current, 0.0));
            *go = Some(Go::Close);
        }
        if row(ui, "rotate-cw", tl!("Rotate view clockwise"), true, false, false).clicked() {
            view.rotate_view(true);
            *go = Some(Go::Close);
        }
    }
    let (theme_icon, theme_label) = theme_choice(app.theme_preference);
    let theme_row = format!("{}: {theme_label}", tl!("Display theme"));
    if row(ui, theme_icon, &theme_row, true, false, true).clicked() {
        *go = Some(Go::Page(PhoneMenu::Theme));
    }
    run(app, ui, "view.full_screen", "maximize", go);

    heading(ui, tl!("Menu"));
    for (menu, icon, label) in MENUS {
        if row(ui, icon, tl!(label), true, false, true).clicked() {
            *go = Some(Go::Page(PhoneMenu::Commands(menu)));
        }
    }
    if row(ui, "settings", tl!("Preferences…"), true, false, false).clicked() {
        app.dialog = Some(Dialog::Preferences);
        *go = Some(Go::Close);
    }
}

fn command_page(app: &mut PrckopisApp, ui: &mut egui::Ui, menu: &'static str, go: &mut Option<Go>) {
    for spec in commands::menu(menu) {
        if spec.id == "file.open_recent" {
            let label = crate::i18n::menu_label(spec.id, &commands::current_label(spec, &app.session, None));
            if row(ui, spec.icon, &label, !app.recent.is_empty(), false, true).clicked() {
                *go = Some(Go::Page(PhoneMenu::Recent));
            }
            continue;
        }
        run(app, ui, spec.id, spec.icon, go);
    }
    if menu == "View" {
        if row(ui, "panel-right", tl!("Side panels"), app.active.is_some(), false, true).clicked() {
            *go = Some(Go::Page(PhoneMenu::Panels));
        }
        let (icon, label) = theme_choice(app.theme_preference);
        let theme_row = format!("{}: {label}", tl!("Display theme"));
        if row(ui, icon, &theme_row, true, false, true).clicked() {
            *go = Some(Go::Page(PhoneMenu::Theme));
        }
    }
}

fn recent_page(app: &mut PrckopisApp, ui: &mut egui::Ui, go: &mut Option<Go>) {
    let mut open = None;
    for r in &app.recent {
        if row(ui, "file-text", &r.name, true, false, false).clicked() {
            open = Some(r.path.clone());
        }
    }
    if let Some(p) = open {
        app.open_recent(&p);
        *go = Some(Go::Close);
        return;
    }
    if !app.recent.is_empty() {
        ui.add_space(8.0);
        if row(ui, "trash-2", tl!("Clear Recent Files"), true, false, false).clicked() {
            app.execute("file.clear_recent");
            *go = Some(Go::Page(PhoneMenu::Root));
        }
    }
}

fn documents_page(app: &mut PrckopisApp, ui: &mut egui::Ui, go: &mut Option<Go>) {
    if row(ui, "house", tl!("Home"), true, app.active.is_none(), false).clicked() {
        app.active = None;
        app.combine_tab.focused = false;
        *go = Some(Go::Close);
    }
    let names: Vec<(prckopis_engine::DocId, String, bool)> =
        app.views.iter().filter_map(|v| app.session.get(v.id).map(|d| (v.id, d.display_name(), d.dirty))).collect();
    let mut close = None;
    for (i, (id, name, dirty)) in names.iter().enumerate() {
        let shown = if *dirty { format!("{name} •") } else { name.clone() };
        let resp = row(ui, "file-text", &shown, true, app.active == Some(i), false);
        if resp.clicked() {
            app.active = Some(i);
            *go = Some(Go::Close);
        }
        // A long press closes it (asking first when it has unsaved changes).
        resp.context_menu(|ui| {
            if ui.button(tl!("Close Tab")).clicked() {
                close = Some(*id);
                ui.close();
            }
        });
    }
    if let Some(id) = close {
        app.tab_action(crate::tab_context::Action::Close(id));
    }
}

fn panels_page(app: &mut PrckopisApp, ui: &mut egui::Ui, go: &mut Option<Go>) {
    let has_doc = app.active.is_some();
    for (p, icon, label) in PANELS {
        let showing = app.right == Some(p);
        if row(ui, icon, tl!(label), has_doc, showing, false).clicked() {
            app.left_open = false;
            app.choose_right_panel(if showing { None } else { Some(p) });
            *go = Some(Go::Close);
        }
    }
}

fn theme_choice(p: ThemePreference) -> (&'static str, &'static str) {
    match p {
        ThemePreference::System => ("monitor", tl!("Use system setting")),
        ThemePreference::Light => ("sun", tl!("Light gray")),
        ThemePreference::Dark => ("moon", tl!("Dark gray")),
    }
}

fn theme_page(app: &mut PrckopisApp, ui: &mut egui::Ui, go: &mut Option<Go>) {
    for (p, command) in
        [(ThemePreference::System, "view.theme.system"), (ThemePreference::Light, "view.theme.light"), (ThemePreference::Dark, "view.theme.dark")]
    {
        let (icon, label) = theme_choice(p);
        if row(ui, icon, label, true, app.theme_preference == p, false).clicked() {
            app.execute(command);
            *go = Some(Go::Close);
        }
    }
}
