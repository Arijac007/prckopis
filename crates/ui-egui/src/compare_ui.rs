//! Compare files: choose the older version, then the Compare panel lists the differences
//! (replaced blue, inserted green, deleted red), shades them on the newer document's pages, and
//! offers a report or the differences as comments. Side by side, the older document is shown
//! beside the newer one, its own changes shaded, the two scrolling together page for page.

use egui::Color32;
use prckopis_engine::DocId;
use prckopis_engine::compare::{Comparison, Kind};

use crate::theme::{self, Tokens};
use crate::{PrckopisApp, RightPanel, widgets};

/// The last comparison.
pub struct CompareState {
    pub old: DocId,
    pub new: DocId,
    pub result: Comparison,
    /// Regions that look different (page, user-space box), page n against page n.
    pub visual: Vec<(usize, [f64; 4])>,
    pub selected: Option<usize>,
    /// Show the older document beside the newer one (synchronised scrolling).
    pub side_by_side: bool,
}

/// Visual differences are shaded orange.
pub const VISUAL: Color32 = Color32::from_rgb(0xF0, 0x8C, 0x1A);

/// What the panel asks for.
pub enum PanelAction {
    Select(usize),
    Mark,
    Report,
    Clear,
    SideBySide(bool),
}

pub fn colour(kind: Kind) -> Color32 {
    let [r, g, b] = prckopis_engine::compare::colour(kind);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The Compare Files dialog: pick the older document. Returns (compare, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PrckopisApp, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Compare Files")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((_, new)) = app.active_ids() else { return (false, true) };
    let others: Vec<(DocId, String)> =
        app.views.iter().filter(|v| v.id != new).filter_map(|v| app.session.get(v.id).map(|d| (d.id, d.name.clone()))).collect();
    if app.compare_old.is_none_or(|o| !others.iter().any(|(id, _)| *id == o)) {
        app.compare_old = others.first().map(|(id, _)| *id);
    }
    let new_name = app.session.get(new).map(|d| d.name.clone()).unwrap_or_default();
    egui::Grid::new("compare-files").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label(tl!("Old file:"));
        let shown =
            others.iter().find(|(id, _)| Some(*id) == app.compare_old).map_or(tl!("Open the older version first").to_string(), |(_, n)| n.clone());
        egui::ComboBox::from_id_salt("compare-old").selected_text(shown).width(crate::widgets::fit_width(ui, 320.0)).show_ui(ui, |ui| {
            for (id, n) in &others {
                ui.selectable_value(&mut app.compare_old, Some(*id), n);
            }
        });
        ui.end_row();
        ui.label(tl!("New file:"));
        ui.label(egui::RichText::new(new_name).strong());
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Text is compared word by word; the differences are shown on the new file.")).small().color(t.text_muted));
    ui.add_space(10.0);
    let (mut go, mut cancel) = (false, false);
    crate::widgets::row(ui, |ui| {
        crate::widgets::trailing(ui, |ui| {
            if ui.add_enabled_ui(app.compare_old.is_some(), |ui| widgets::pill_button(ui, tl!("Compare"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// The Compare panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &Option<CompareState>, id: DocId) -> Option<PanelAction> {
    let Some(s) = state.as_ref().filter(|s| s.new == id) else {
        ui.label(egui::RichText::new(tl!("Compare this file with an older version: Compare files… in All tools.")).color(t.text_muted));
        return None;
    };
    let mut action = None;
    let c = &s.result;
    if c.identical() {
        ui.label(tl!("No differences in the text."));
    }
    ui.horizontal_wrapped(|ui| {
        for k in [Kind::Replaced, Kind::Inserted, Kind::Deleted] {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(dot, egui::CornerRadius::same(2), colour(k));
            ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &c.count(k).to_string()), ("kind", tl!(k.label()))]));
        }
        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(dot, egui::CornerRadius::same(2), VISUAL);
        ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &s.visual.len().to_string()), ("kind", tl!("Visual"))]));
    });
    ui.add_space(6.0);
    crate::widgets::row(ui, |ui| {
        if ui.button(tl!("Mark as comments")).clicked() {
            action = Some(PanelAction::Mark);
        }
        if ui.button(tl!("Report…")).clicked() {
            action = Some(PanelAction::Report);
        }
        if ui.button(tl!("Clear")).clicked() {
            action = Some(PanelAction::Clear);
        }
    });
    let mut side = s.side_by_side;
    if ui
        .add(widgets::checkbox(&mut side, tl!("Side by side")))
        .on_hover_text(tl!("Show the old file beside the new one, scrolling together"))
        .changed()
    {
        action = Some(PanelAction::SideBySide(side));
    }
    ui.separator();
    for (i, ch) in c.changes.iter().enumerate().take(2000) {
        let text = match ch.kind {
            Kind::Replaced => format!("\"{}\" → \"{}\"", ch.old.text, ch.new.text),
            Kind::Inserted => format!("+ \"{}\"", ch.new.text),
            Kind::Deleted => format!("− \"{}\"", ch.old.text),
        };
        let selected = s.selected == Some(i);
        let r = ui.push_id(i, |ui| {
            crate::widgets::row(ui, |ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(4.0, 34.0), egui::Sense::hover());
                ui.painter().rect_filled(bar, egui::CornerRadius::same(2), colour(ch.kind));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt(
                            tl!("{kind} · page {p}"),
                            &[("kind", tl!(ch.kind.label())), ("p", &(ch.new.page + 1).to_string())],
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                    ui.selectable_label(selected, text)
                })
                .inner
            })
            .inner
        });
        if r.inner.clicked() {
            action = Some(PanelAction::Select(i));
        }
    }
    action
}

impl PrckopisApp {
    /// Compare the chosen older document with the active one and show the differences.
    pub fn run_compare(&mut self) {
        let Some((i, new)) = self.active_ids() else { return };
        let Some(old) = self.compare_old else { return };
        match self.session.compare(old, new) {
            Ok(result) => {
                let visual = self.session.compare_visual(old, new, 72.0).unwrap_or_default();
                self.views[i].compare_marks = result
                    .changes
                    .iter()
                    .flat_map(|ch| ch.new.rects.iter().map(move |r| (ch.new.page, r.map(|v| v as f32), colour(ch.kind))))
                    .collect();
                // Visual differences not already covered by a text change.
                let text_marks = self.views[i].compare_marks.clone();
                for (p, r) in &visual {
                    let r = r.map(|v| v as f32);
                    let covered = text_marks.iter().any(|(mp, m, _)| *mp == *p && m[0] <= r[2] && r[0] <= m[2] && m[1] <= r[3] && r[1] <= m[3]);
                    if !covered {
                        self.views[i].compare_marks.push((*p, r, VISUAL));
                    }
                }
                if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
                    v.compare_marks = result
                        .changes
                        .iter()
                        .flat_map(|ch| ch.old.rects.iter().map(move |r| (ch.old.page, r.map(|v| v as f32), colour(ch.kind))))
                        .collect();
                }
                let n = result.changes.len();
                let side_by_side = self.compare.as_ref().is_some_and(|c| c.side_by_side);
                self.compare = Some(crate::compare_ui::CompareState { old, new, result, visual, selected: None, side_by_side });
                self.right = Some(RightPanel::Compare);
                self.notify(if n == 0 {
                    tl!("The text is the same").to_string()
                } else if n == 1 {
                    tl!("1 difference found").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} differences found"), &[("n", &n.to_string())])
                });
            }
            Err(e) => self.notify_error(e),
        }
    }

    pub(crate) fn compare_action(&mut self, index: usize, a: PanelAction) {
        let Some(s) = self.compare.as_mut() else { return };
        let (old, new) = (s.old, s.new);
        match a {
            PanelAction::Select(i) => {
                s.selected = Some(i);
                let Some(ch) = s.result.changes.get(i) else { return };
                let rect = ch.new.rects.first().copied().or(ch.new.near).unwrap_or_default();
                let page = ch.new.page;
                if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
                    let old_rect = ch.old.rects.first().copied().or(ch.old.near).unwrap_or_default();
                    v.flash = Some((ch.old.page, old_rect.map(|x| x as f32), 0.0));
                }
                let v = &mut self.views[index];
                v.go_to_page(page);
                v.flash = Some((page, rect.map(|x| x as f32), 0.0));
            }
            PanelAction::Mark => match self.session.mark_differences(old, new) {
                Ok(n) => {
                    if let Some(info) = self.session.get(new).map(|d| d.info.clone()) {
                        self.views[index].document_changed(&info);
                    }
                    self.notify(if n == 1 {
                        tl!("Added 1 comment").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Added {n} comments"), &[("n", &n.to_string())])
                    });
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Report => match self.session.compare_report(old, new) {
                Ok(bytes) => {
                    if let Err(e) = self.open_bytes("Compare Report.pdf", None, bytes.to_vec()) {
                        self.notify_error(e);
                    }
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Clear => {
                self.compare = None;
                self.views[index].compare_marks.clear();
                if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
                    v.compare_marks.clear();
                }
                self.right = None;
            }
            PanelAction::SideBySide(on) => s.side_by_side = on,
        }
    }

    /// The older document's view when the active one is shown side by side with it.
    pub(crate) fn compare_companion(&self, index: usize) -> Option<usize> {
        let s = self.compare.as_ref().filter(|s| s.side_by_side)?;
        let active = self.views.get(index)?;
        if active.id != s.new {
            return None;
        }
        self.views.iter().position(|v| v.id == s.old && self.session.get(v.id).is_some())
    }

    /// The Canvas with Compare side by side: the older document on the left, the newer one
    /// (fully editable) on the right, each following the other's scrolling page for page.
    pub(crate) fn compare_split(&mut self, index: usize, old: usize, ui: &mut egui::Ui) {
        let Some(s) = self.compare.as_ref() else { return };
        let (Some(old_doc), Some(new_doc)) = (self.session.get(s.old), self.session.get(s.new)) else { return };
        let (old_info, old_pages, new_pages) = (old_doc.info.clone(), old_doc.info.pages.len(), new_doc.info.pages.len());
        let (old_name, new_name) = (old_doc.name.clone(), new_doc.name.clone());
        // What the newer side showed last frame, as an old page and the point to keep in the middle.
        let follow = self.views[index].reading_position().and_then(|(p, f)| s.result.old_page_for(p, old_pages).map(|o| (o, f)));
        let zoom = self.views[index].zoom;
        let t = Tokens::get(ui.ctx());
        let whole = ui.available_rect_before_wrap();
        // Which side is which, above each half.
        const CAPTION: f32 = 24.0;
        let full = egui::Rect::from_min_max(egui::pos2(whole.min.x, whole.min.y + CAPTION), whole.max);
        let half = (full.width() / 2.0).floor();
        for (x, label) in [
            (whole.min.x, crate::i18n::fmt(tl!("Old: {name}"), &[("name", &old_name)])),
            (whole.min.x + half, crate::i18n::fmt(tl!("New: {name}"), &[("name", &new_name)])),
        ] {
            let cell = egui::Rect::from_min_size(egui::pos2(x, whole.min.y), egui::vec2(half, CAPTION));
            ui.painter().rect_filled(cell, 0.0, t.panel);
            let galley = ui.painter().layout(label, theme::regular(12.5), t.text_muted, (half - 24.0).max(40.0));
            ui.painter().galley(egui::pos2(cell.min.x + 12.0, cell.center().y - galley.size().y / 2.0), galley, t.text_muted);
        }
        let left = egui::Rect::from_min_size(full.min, egui::vec2(half - 1.0, full.height()));
        let right = egui::Rect::from_min_max(egui::pos2(full.min.x + half + 1.0, full.min.y), full.max);
        ui.painter().vline(full.min.x + half, whole.y_range(), egui::Stroke::new(1.0, t.border));
        let scrolled = ui
            .scope_builder(egui::UiBuilder::new().max_rect(left).id_salt("compare-old-side"), |ui| {
                crate::canvas::compare_side_pane(&mut self.views[old], &old_info, zoom, follow, ui)
            })
            .inner;
        if let Some((p, f)) = scrolled
            && let Some(s) = self.compare.as_ref()
            && let Some(n) = s.result.new_page_for(p, new_pages)
        {
            self.views[index].centre_on(n, f);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(right).id_salt("compare-new-side"), |ui| crate::canvas::document_area(self, index, ui));
    }
}
