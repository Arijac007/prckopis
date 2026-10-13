//! Print production ▸ Preflight: choose a profile (PDF/A-2b, PDF/A-3b, PDF/X-4), analyze the
//! document, read what it found grouped by problem with the pages it is on, fix what can be fixed,
//! and save a report.

use egui::{Align, Layout};
use prckopis_engine::preflight::{Profile, Results};

use crate::theme::{self, Tokens};
use crate::{PrckopisApp, widgets};

/// Page links shown for one problem before the rest are summed up as "…".
const PAGE_LINKS: usize = 24;

/// The dialog's state: the chosen profile and the last results.
#[derive(Clone, Debug, PartialEq)]
pub struct PreflightState {
    pub profile: Profile,
    pub results: Option<Results>,
}

impl Default for PreflightState {
    fn default() -> Self {
        PreflightState { profile: Profile::PdfX4, results: None }
    }
}

impl PrckopisApp {
    /// Analyze: run the chosen profile over the active document.
    pub fn preflight_analyze(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let profile = self.preflight.profile;
        self.preflight.results = self.session.get(id).map(|d| d.preflight(profile));
    }

    /// Fix: apply the profile's fixups (one undoable step), then analyze again.
    pub fn preflight_fix(&mut self) {
        let profile = self.preflight.profile;
        if self.apply_edit(prckopis_engine::Edit::PreflightFix { profile }) {
            self.preflight_analyze();
            let left = self.preflight.results.as_ref().map_or(0, |r| r.issues.len());
            self.notify(if left == 0 {
                crate::i18n::fmt(tl!("The document now conforms to {level}; save it to keep the changes"), &[("level", profile.label())])
            } else if left == 1 {
                crate::i18n::fmt(tl!("Fixed what could be fixed; 1 problem remains"), &[])
            } else {
                crate::i18n::fmt(tl!("Fixed what could be fixed; {n} problems remain"), &[("n", &left.to_string())])
            });
        }
    }

    /// Save report…: the results as text, in the interface's language.
    pub fn preflight_save_report(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(results) = self.preflight.results.clone() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (y, m, d) = self.session.today();
        let text = prckopis_engine::preflight::report_text(&results, &doc.name, &format!("{y}-{m:02}-{d:02}"), &crate::i18n::words);
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let title = tl!("Save the Preflight report").to_string();
        self.write_files(&[(format!("{stem} Preflight Report.txt"), std::sync::Arc::new(text.into_bytes()))], &title);
    }

    /// Turn to a page a problem is on.
    fn preflight_show_page(&mut self, page: usize) {
        if let Some((index, _)) = self.active_ids()
            && let Some(view) = self.views.get_mut(index)
        {
            view.go_to_page(page);
        }
    }
}

/// `text` with its first letter a capital, to stand after a `·` like "Can be fixed" does.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

enum Action {
    Analyze,
    Fix,
    Report,
    Page(usize),
}

/// Returns `true` to close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PrckopisApp, t: &Tokens) -> bool {
    let words = &crate::i18n::words;
    ui.label(egui::RichText::new(tl!("Preflight")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let before = app.preflight.profile;
    // Label and choice wrap onto two lines on a phone rather than pushing the dialog wider.
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Profile:"));
        let width = ui.available_width().clamp(120.0, 300.0);
        egui::ComboBox::from_id_salt("preflight-profile").width(width).selected_text(app.preflight.profile.described(words)).show_ui(ui, |ui| {
            for p in Profile::ALL {
                ui.selectable_value(&mut app.preflight.profile, p, p.described(words));
            }
        });
    });
    if app.preflight.profile != before {
        app.preflight.results = None;
    }
    ui.add_space(6.0);
    let mut action = None;
    let narrow = widgets::narrow(ui.ctx());
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Over half the window's height: a results list is read, not glanced at.
        let height = (ui.ctx().content_rect().height() * 0.55).max(200.0);
        egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
            ui.set_min_height(120.0);
            match &app.preflight.results {
                None => {
                    ui.label(egui::RichText::new(tl!("Analyze to see what the document needs for this profile.")).color(t.text_muted));
                }
                Some(r) if r.passed() => {
                    ui.label(crate::i18n::fmt(tl!("No problems found: the document conforms to {level}."), &[("level", r.profile.label())]));
                }
                Some(r) => {
                    let found = words("Problems found: {}").replacen("{}", &r.issues.len().to_string(), 1);
                    let fixable = crate::i18n::fmt(tl!("Can be fixed: {n}"), &[("n", &r.fixable().to_string())]);
                    ui.label(egui::RichText::new(format!("{found} · {fixable}")).strong());
                    for g in r.groups() {
                        ui.add_space(6.0);
                        // The checker hands the wording over apart from the document's own name, so
                        // the problem reads in this language and the name as it is.
                        let said = if g.count > 1 { format!("{} (×{})", g.issue.said(words), g.count) } else { g.issue.said(words) };
                        ui.label(said);
                        if !g.pages.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                // A line of small links, not a row of 30-point fields.
                                ui.spacing_mut().interact_size.y = 16.0;
                                for p in g.pages.iter().take(PAGE_LINKS) {
                                    let label = crate::i18n::fmt(tl!("Page {label}"), &[("label", &(p + 1).to_string())]);
                                    // A page link: the accent colour, no frame.
                                    if ui.add(egui::Button::new(egui::RichText::new(label).small().color(t.accent)).frame(false)).clicked() {
                                        action = Some(Action::Page(*p));
                                    }
                                }
                                if g.pages.len() > PAGE_LINKS {
                                    ui.label(egui::RichText::new("…").small().color(t.text_muted));
                                }
                            });
                        }
                        let fix = if g.issue.fixable {
                            words("Can be fixed")
                        } else {
                            capitalized(tl!("  · not fixable here").trim_start().trim_start_matches('·').trim())
                        };
                        ui.label(egui::RichText::new(format!("{} · {fix}", g.issue.citation())).small().color(t.text_muted));
                    }
                }
            }
        });
    });
    ui.add_space(10.0);
    let mut close = false;
    let has_results = app.preflight.results.is_some();
    let can_fix = app.preflight.results.as_ref().is_some_and(|r| r.fixable() > 0);
    let mut buttons = |ui: &mut egui::Ui| {
        if can_fix && widgets::pill_button(ui, tl!("Fix"), true).clicked() {
            action = Some(Action::Fix);
        }
        if widgets::pill_button(ui, tl!("Analyze"), !can_fix).clicked() {
            action = Some(Action::Analyze);
        }
        if has_results && widgets::pill_button(ui, tl!("Save report…"), false).clicked() {
            action = Some(Action::Report);
        }
        if widgets::pill_button(ui, tl!("Close"), false).clicked() {
            close = true;
        }
    };
    if narrow {
        // A phone: the buttons wrap instead of running off the side.
        ui.horizontal_wrapped(|ui| buttons(ui));
    } else {
        ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| buttons(ui)));
    }
    match action {
        Some(Action::Analyze) => app.preflight_analyze(),
        Some(Action::Fix) => app.preflight_fix(),
        Some(Action::Report) => app.preflight_save_report(),
        Some(Action::Page(p)) => app.preflight_show_page(p),
        None => {}
    }
    close
}
