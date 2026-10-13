//! Redact a PDF (execution plan M8.5–M8.6): mark text and areas with the Redact tool (drag across
//! text to mark it, drag elsewhere to mark a box), mark whole pages, find text or patterns and
//! mark every match, set the default box colour and overlay text, and apply all marks (with a
//! confirmation, as Acrobat asks) or clear them.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use prckopis_engine::{
    Edit, NewAnnotation, REDACT_PATTERNS, REDACTION_CODE_SETS, RedactPattern, RedactionCodeSet, RedactionCodeSetError, RedactionCodeSets, Rgb, Shape,
    Style, rect_quad,
};
use prckopis_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;
use crate::{PrckopisApp, widgets};

const MARK_RED: Color32 = Color32::from_rgb(0xE3, 0x22, 0x22);

/// Redaction Tool Properties: what new marks look like once applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RedactPrefs {
    /// Box colour (`None` = no box, the content just disappears).
    pub fill: Option<Rgb>,
    pub use_overlay: bool,
    pub overlay: String,
    /// The overlay text is the picked redaction codes instead of `overlay`.
    pub use_code: bool,
    pub code_set: CodeSetChoice,
    /// The picked codes of `code_set`.
    pub codes: Vec<String>,
    /// The user's own code sets (Redaction Code Editor), kept in the preferences.
    pub code_sets: RedactionCodeSets,
    /// Overlay text font, size (0 = auto), colour, alignment and repetition.
    pub look: prckopis_engine::OverlayLook,
}

/// Which code set the redaction codes come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeSetChoice {
    BuiltIn(RedactionCodeSet),
    /// A user-defined set, by name.
    Custom(String),
}

impl Default for RedactPrefs {
    fn default() -> Self {
        Self {
            fill: Some([0.0, 0.0, 0.0]),
            use_overlay: false,
            overlay: String::new(),
            use_code: false,
            // Infallible: a non-empty constant array.
            code_set: CodeSetChoice::BuiltIn(REDACTION_CODE_SETS[0]),
            codes: Vec::new(),
            code_sets: RedactionCodeSets::default(),
            look: Default::default(),
        }
    }
}

impl RedactPrefs {
    pub fn overlay_text(&self) -> String {
        match (self.use_overlay, self.use_code) {
            (false, _) => String::new(),
            (true, false) => self.overlay.clone(),
            // Picked codes the set no longer has (edited meanwhile) are left out.
            (true, true) => prckopis_engine::redaction_overlay(&self.set_codes(), &self.codes),
        }
    }

    /// The codes of the chosen set, with their descriptions (none for the built-in sets).
    pub fn set_entries(&self) -> Vec<(String, String)> {
        match &self.code_set {
            CodeSetChoice::BuiltIn(s) => s.codes.iter().map(|c| (c.to_string(), String::new())).collect(),
            CodeSetChoice::Custom(name) => {
                self.code_sets.get(name).map(|s| s.codes.iter().map(|c| (c.code.clone(), c.description.clone())).collect()).unwrap_or_default()
            }
        }
    }

    pub fn set_codes(&self) -> Vec<String> {
        self.set_entries().into_iter().map(|e| e.0).collect()
    }

    pub fn set_name(&self) -> String {
        match &self.code_set {
            CodeSetChoice::BuiltIn(s) => s.name.to_string(),
            CodeSetChoice::Custom(name) => name.clone(),
        }
    }

    /// Choose the set to take codes from; the picked codes are cleared when it changes.
    pub fn choose_set(&mut self, set: CodeSetChoice) {
        if self.code_set != set {
            self.code_set = set;
            self.codes.clear();
        }
    }

    fn is_chosen(&self, name: &str) -> bool {
        matches!(&self.code_set, CodeSetChoice::Custom(n) if n.trim().to_lowercase() == name.trim().to_lowercase())
    }

    /// Add imported code sets (renamed when their names are taken); returns how many were added.
    pub fn import_code_sets(&mut self, bytes: &[u8]) -> Result<(usize, usize), RedactionCodeSetError> {
        let imported = prckopis_engine::import_redaction_codes(bytes)?;
        let total = imported.sets.len();
        let added = self.code_sets.merge(imported.sets).len();
        Ok((added, imported.skipped + (total - added)))
    }

    pub fn mark(&self, page: usize, quads: Vec<[f64; 8]>, author: &str) -> Edit {
        let shape = Shape::Redact { quads, overlay: self.overlay_text(), look: self.look };
        let mut style = Style::default_for(&shape);
        style.fill = self.fill;
        Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: author.to_string() })
    }
}

/// The Redaction Code Editor's fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeEditorDraft {
    /// The set being edited, by name.
    pub set: Option<String>,
    /// The set name field (New set, Rename).
    pub set_name: String,
    /// The code selected in the list.
    pub code: Option<String>,
    pub code_text: String,
    pub description: String,
    /// The last edit's problem, shown under the fields.
    pub error: Option<String>,
}

/// What the Redaction Code Editor asks for besides editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeEditorAction {
    pub done: bool,
    pub import: bool,
    /// Export this set, by name.
    pub export: Option<String>,
}

fn code_set_error(e: &RedactionCodeSetError) -> String {
    use prckopis_engine::RedactionCodeSetError as E;
    match e {
        E::EmptyName => tl!("Type a name first."),
        E::DuplicateSet(_) => tl!("That name is already used."),
        E::EmptyCode => tl!("Type a code first."),
        E::DuplicateCode(_) => tl!("This set already has that code."),
        E::NameTooLong | E::CodeTooLong | E::DescriptionTooLong => tl!("The name, code or description is too long."),
        E::TooManySets | E::TooManyCodes | E::TooLarge => tl!("There are too many code sets or codes."),
        E::NoSuchSet(_) | E::NoSuchCode(_) | E::Malformed(_) => tl!("That set or code no longer exists."),
    }
    .to_string()
}

/// The Redaction Code Editor (Redaction properties ▸ Edit codes…): create, rename and delete
/// code sets, add, change and remove their codes, and import or export them.
pub(crate) fn codes_body(ui: &mut egui::Ui, p: &mut RedactPrefs, d: &mut CodeEditorDraft, t: &Tokens) -> CodeEditorAction {
    let mut action = CodeEditorAction::default();
    crate::widgets::set_fit_width(ui, 480.0);
    ui.label(egui::RichText::new(tl!("Redaction Code Editor")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    // The selected set may have gone (deleted, or a stale name).
    if d.set.as_deref().is_some_and(|s| p.code_sets.get(s).is_none()) {
        d.set = None;
    }
    if d.set.is_none() {
        d.set = p.code_sets.sets().first().map(|s| s.name.clone());
    }
    ui.label(egui::RichText::new(tl!("Code sets")).strong());
    egui::ScrollArea::vertical().id_salt("redaction-code-sets").max_height(110.0).auto_shrink([false, true]).show(ui, |ui| {
        if p.code_sets.is_empty() {
            ui.label(egui::RichText::new(tl!("No code sets yet. Type a name and choose New set.")).color(t.text_faint));
        }
        for s in p.code_sets.sets() {
            let on = d.set.as_deref() == Some(s.name.as_str());
            if ui.selectable_label(on, &s.name).clicked() && !on {
                d.set = Some(s.name.clone());
                d.set_name = s.name.clone();
                d.code = None;
            }
        }
    });
    let mut err: Option<RedactionCodeSetError> = None;
    crate::widgets::row(ui, |ui| {
        let l = ui.label(tl!("Name:"));
        ui.add(crate::widgets::line(&mut d.set_name).desired_width(crate::widgets::fit_width(ui, 180.0))).labelled_by(l.id);
        if ui.button(tl!("New set")).clicked() {
            match p.code_sets.add_set(&d.set_name) {
                Ok(name) => {
                    d.set = Some(name);
                    d.code = None;
                }
                Err(e) => err = Some(e),
            }
        }
        let selected = d.set.clone();
        if ui.add_enabled(selected.is_some(), egui::Button::new(tl!("Rename"))).clicked()
            && let Some(old) = selected.clone()
        {
            match p.code_sets.rename_set(&old, &d.set_name) {
                Ok(name) => {
                    if p.is_chosen(&old) {
                        p.code_set = CodeSetChoice::Custom(name.clone());
                    }
                    d.set = Some(name);
                }
                Err(e) => err = Some(e),
            }
        }
        if ui.add_enabled(selected.is_some(), egui::Button::new(tl!("Delete"))).clicked()
            && let Some(old) = selected
        {
            if let Err(e) = p.code_sets.remove_set(&old) {
                err = Some(e);
            } else {
                if p.is_chosen(&old) {
                    p.choose_set(CodeSetChoice::BuiltIn(REDACTION_CODE_SETS[0]));
                }
                d.set = None;
                d.code = None;
            }
        }
    });
    ui.separator();
    let set = d.set.clone();
    ui.add_enabled_ui(set.is_some(), |ui| {
        ui.label(egui::RichText::new(tl!("Codes in this set")).strong());
        let entries: Vec<(String, String)> = set
            .as_deref()
            .and_then(|s| p.code_sets.get(s))
            .map(|s| s.codes.iter().map(|c| (c.code.clone(), c.description.clone())).collect())
            .unwrap_or_default();
        egui::ScrollArea::vertical().id_salt("redaction-codes").max_height(150.0).auto_shrink([false, true]).show(ui, |ui| {
            for (code, description) in &entries {
                let on = d.code.as_deref() == Some(code.as_str());
                let text = if description.is_empty() { code.clone() } else { format!("{code} — {description}") };
                if ui.selectable_label(on, text).clicked() {
                    d.code = Some(code.clone());
                    d.code_text = code.clone();
                    d.description = description.clone();
                }
            }
        });
        egui::Grid::new("redaction-code-fields").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            let l = ui.label(tl!("Code:"));
            ui.add(crate::widgets::line(&mut d.code_text).desired_width(crate::widgets::fit_width(ui, 300.0))).labelled_by(l.id);
            ui.end_row();
            let l = ui.label(tl!("Description:"));
            ui.add(crate::widgets::line(&mut d.description).desired_width(crate::widgets::fit_width(ui, 300.0))).labelled_by(l.id);
            ui.end_row();
        });
        crate::widgets::row(ui, |ui| {
            let Some(s) = set.clone() else { return };
            if ui.button(tl!("Add")).clicked() {
                match p.code_sets.add_code(&s, &d.code_text, &d.description) {
                    Ok(()) => {
                        d.code = p.code_sets.get(&s).and_then(|x| x.codes.last()).map(|c| c.code.clone());
                    }
                    Err(e) => err = Some(e),
                }
            }
            let picked = d.code.clone();
            if ui.add_enabled(picked.is_some(), egui::Button::new(tl!("Update"))).clicked()
                && let Some(old) = picked.clone()
            {
                let at = p.code_sets.get(&s).and_then(|x| x.codes.iter().position(|c| c.code == old));
                match p.code_sets.edit_code(&s, &old, &d.code_text, &d.description) {
                    Ok(()) => {
                        // The code as stored (trimmed, on one line), in the same place.
                        let new = at.and_then(|i| p.code_sets.get(&s).and_then(|x| x.codes.get(i))).map(|c| c.code.clone());
                        if let Some(new) = &new
                            && p.is_chosen(&s)
                        {
                            for c in p.codes.iter_mut().filter(|c| **c == old) {
                                c.clone_from(new);
                            }
                        }
                        d.code = new;
                    }
                    Err(e) => err = Some(e),
                }
            }
            if ui.add_enabled(picked.is_some(), egui::Button::new(tl!("Remove"))).clicked()
                && let Some(old) = picked
            {
                match p.code_sets.remove_code(&s, &old) {
                    Ok(()) => {
                        if p.is_chosen(&s) {
                            p.codes.retain(|c| *c != old);
                        }
                        d.code = None;
                    }
                    Err(e) => err = Some(e),
                }
            }
        });
    });
    match err {
        Some(e) => d.error = Some(code_set_error(&e)),
        None if ui.ctx().input(|i| i.pointer.any_click()) => d.error = None,
        None => {}
    }
    if let Some(e) = &d.error {
        ui.label(egui::RichText::new(e).color(t.text_muted));
    }
    ui.add_space(12.0);
    crate::widgets::row(ui, |ui| {
        action.import = ui.button(tl!("Import…")).clicked();
        if ui.add_enabled(set.is_some(), egui::Button::new(tl!("Export…"))).clicked() {
            action.export = set.clone();
        }
        crate::widgets::trailing(ui, |ui| {
            action.done = widgets::pill_button(ui, tl!("Done"), true).clicked();
        });
    });
    action
}

/// Redact pages: which pages to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct PagesDraft {
    pub current: bool,
    pub from: usize,
    pub to: usize,
}

impl Default for PagesDraft {
    fn default() -> Self {
        Self { current: true, from: 1, to: 1 }
    }
}

/// What Find text and redact looks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    Phrase,
    /// Every word or phrase of a list, one per line.
    Words,
    Patterns,
}

/// Find text and redact.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDraft {
    pub mode: SearchMode,
    pub text: String,
    /// The word list, one word or phrase per line.
    pub words: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub pattern: RedactPattern,
    /// The result of the last search (matches marked), shown in the dialog.
    pub found: Option<usize>,
}

impl Default for SearchDraft {
    fn default() -> Self {
        Self {
            mode: SearchMode::Phrase,
            text: String::new(),
            words: String::new(),
            case_sensitive: false,
            whole_words: false,
            pattern: RedactPattern::Phone,
            found: None,
        }
    }
}

/// The largest word list file Import list accepts.
pub(crate) const MAX_WORD_LIST_BYTES: usize = 1 << 20;

/// A box being drawn with the Redact tool: (page, start).
pub type AreaDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Redact tool input on a page. Presses on text are left to text selection (the selection is
/// marked when it ends, see [`after_text`]); presses elsewhere draw a box. Returns `true` when
/// the gesture is a box.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    over_text: impl Fn(Pos2) -> bool,
    view: &mut DocView,
) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
    if let Some((dp, start)) = view.redact_drag
        && dp == page
    {
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            view.redact_drag = None;
            let end = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, end).intersect(xf.rect);
            if r.width() >= 3.0 && r.height() >= 3.0 {
                let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
                view.pending_redaction = Some((page, vec![rect_quad([a[0], a[1], b[0], b[1]])]));
            }
        }
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    if !over_text(p) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        if !over_text(origin) {
            view.redact_drag = Some((page, origin));
            return true;
        }
    }
    false
}

/// A finished text selection with the Redact tool becomes a mark.
pub(crate) fn after_text(resp: &egui::Response, page: usize, info: &DocInfo, view: &mut DocView) {
    if !(resp.drag_stopped() || resp.double_clicked()) || view.redact_drag.is_some() {
        return;
    }
    if let Some((p, quads)) = view.selection_quads(info).filter(|(p, _)| *p == page) {
        view.clear_selection();
        view.pending_redaction = Some((p, quads));
    }
}

/// The box being drawn.
pub(crate) fn paint(ui: &egui::Ui, painter: &egui::Painter, page: usize, view: &DocView) {
    if let (Some((dp, start)), Some(p)) = (view.redact_drag, ui.input(|i| i.pointer.hover_pos()))
        && dp == page
    {
        let r = Rect::from_two_pos(start, p);
        painter.rect_filled(r, CornerRadius::ZERO, MARK_RED.gamma_multiply(0.12));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, MARK_RED), egui::StrokeKind::Inside);
    }
}

impl PrckopisApp {
    /// Mark every match of the search draft on every page; returns how many were marked.
    pub fn redact_search(&mut self) -> usize {
        let Some((_, id)) = self.active_ids() else { return 0 };
        let Some(doc) = self.session.get(id) else { return 0 };
        let d = self.redact_search.clone();
        let needles = match d.mode {
            SearchMode::Phrase if !d.text.trim().is_empty() => vec![d.text.clone()],
            SearchMode::Words => prckopis_engine::redact_word_list(&d.words),
            SearchMode::Phrase | SearchMode::Patterns => Vec::new(),
        };
        if d.mode != SearchMode::Patterns && needles.is_empty() {
            return 0;
        }
        let config = prckopis_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = prckopis_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut edits = Vec::new();
        let author = self.comment_prefs.author.clone();
        for (page, page_info) in doc.info.pages.iter().enumerate() {
            let out = r.render(prckopis_render::RenderRequest { page, kind: prckopis_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = out.text else { continue };
            let hits = match d.mode {
                SearchMode::Patterns => text.find_with(|c| prckopis_engine::find_pattern(d.pattern, c)),
                _ => needles.iter().flat_map(|w| text.find_opts(w, d.case_sensitive, d.whole_words)).collect(),
            };
            for h in hits {
                let quads: Vec<[f64; 8]> = text.line_rects(h).into_iter().map(|r| page_info.view_rect_to_quad(r)).collect();
                if !quads.is_empty() {
                    edits.push(self.redact_prefs.mark(page, quads, &author));
                }
            }
        }
        let n = edits.len();
        if n > 0 {
            self.apply_edit(Edit::Batch { label: "Mark for redaction".into(), edits });
        }
        n
    }

    /// Mark the pages of the Redact pages draft.
    pub fn redact_pages(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let n = doc.info.pages.len();
        let d = self.redact_pages_draft.clone();
        let pages: Vec<usize> = if d.current { vec![self.views[i].current] } else { (d.from.max(1) - 1..d.to.min(n)).collect() };
        let author = self.comment_prefs.author.clone();
        let edits: Vec<Edit> = pages
            .iter()
            .map(|&p| {
                let c = doc.info.pages[p].crop;
                self.redact_prefs.mark(p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])], &author)
            })
            .collect();
        match <[Edit; 1]>::try_from(edits) {
            Ok([one]) => {
                self.apply_edit(one);
            }
            Err(edits) if edits.is_empty() => {}
            Err(edits) => {
                self.apply_edit(Edit::Batch { label: "Mark pages for redaction".into(), edits });
            }
        }
    }
}

impl PrckopisApp {
    /// Redaction Code Editor ▸ Import: add the sets of a code-set file (untrusted).
    pub fn import_redaction_codes(&mut self, name: &str, bytes: &[u8]) {
        let before = self.redact_prefs.code_sets.sets().len();
        match self.redact_prefs.import_code_sets(bytes) {
            Ok((added, skipped)) => {
                let n = added.to_string();
                if skipped > 0 {
                    self.notify_fmt(
                        "Imported {n} code set(s) from {name}; skipped {k} unreadable or repeated entries.",
                        &[("n", &n), ("name", name), ("k", &skipped.to_string())],
                    );
                } else {
                    self.notify_fmt("Imported {n} code set(s) from {name}.", &[("n", &n), ("name", name)]);
                }
                // Show the first imported set in the editor.
                if let Some(s) = self.redact_prefs.code_sets.sets().get(before) {
                    self.redact_code_editor = CodeEditorDraft { set: Some(s.name.clone()), set_name: s.name.clone(), ..Default::default() };
                }
            }
            Err(RedactionCodeSetError::TooLarge) => self.notify_fmt("{name} is too large for a code set file (at most 1 MB).", &[("name", name)]),
            Err(_) => self.notify_fmt("{name} is not a redaction code set file.", &[("name", name)]),
        }
    }

    /// Redaction Code Editor ▸ Export: write one set as a code-set file (asks where;
    /// `save_override` in tests).
    pub fn export_redaction_codes(&mut self, set: &str) {
        let Some(s) = self.redact_prefs.code_sets.get(set) else { return };
        let text = prckopis_engine::export_redaction_codes([s]);
        let file: String = s.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let file = format!("{}.json", if file.trim_matches('_').is_empty() { "redaction-codes" } else { file.as_str() });
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write =
                move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), text.as_bytes()) {
                    Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                    Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
                };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Export…").to_string()).add_filter("JSON", &["json"]).set_file_name(file);
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = crate::editing::download(&file, text.as_bytes()) {
            self.notify_error(e);
        }
    }
}

/// Redact pages ("Mark Page Range"). Returns (apply, cancel).
pub(crate) fn pages_body(ui: &mut egui::Ui, d: &mut PagesDraft, pages: usize, _t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Mark Page Range")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::radio_value(ui, &mut d.current, true, tl!("Current page"));
    crate::widgets::row(ui, |ui| {
        widgets::radio_value(ui, &mut d.current, false, tl!("Pages from"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.from).range(1..=pages));
        ui.label(tl!("to"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.to).range(1..=pages));
        ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &pages.to_string())]));
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    buttons(ui, "OK", true)
}

/// Find text and redact. Returns (search, cancel, import a word list).
pub(crate) fn search_body(ui: &mut egui::Ui, d: &mut SearchDraft, t: &Tokens) -> (bool, bool, bool) {
    crate::widgets::set_fit_width(ui, 420.0);
    ui.label(egui::RichText::new(tl!("Find text and redact")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Phrase, tl!("Single word or phrase"));
    let mut enter = false;
    ui.add_enabled_ui(d.mode == SearchMode::Phrase, |ui| {
        let r = ui.add(crate::widgets::line(&mut d.text).hint_text(tl!("Text to find")).desired_width(crate::widgets::fit_width(ui, 380.0)));
        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    ui.add_space(6.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Words, tl!("Multiple words or phrases"));
    let mut import = false;
    ui.add_enabled_ui(d.mode == SearchMode::Words, |ui| {
        egui::ScrollArea::vertical().id_salt("redact-words").max_height(120.0).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut d.words)
                    .hint_text(tl!("One word or phrase per line"))
                    .desired_width(crate::widgets::fit_width(ui, 380.0))
                    .desired_rows(4),
            );
        });
        crate::widgets::row(ui, |ui| {
            import = ui.button(tl!("Import list…")).clicked();
            let n = prckopis_engine::redact_word_list(&d.words).len();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} word(s) or phrase(s)"), &[("n", &n.to_string())])).small().color(t.text_faint));
        });
    });
    ui.add_enabled_ui(d.mode != SearchMode::Patterns, |ui| {
        ui.add(widgets::checkbox(&mut d.whole_words, tl!("Whole words only")));
        ui.add(widgets::checkbox(&mut d.case_sensitive, tl!("Case-sensitive")));
    });
    ui.add_space(6.0);
    widgets::radio_value(ui, &mut d.mode, SearchMode::Patterns, tl!("Patterns"));
    ui.add_enabled_ui(d.mode == SearchMode::Patterns, |ui| {
        egui::ComboBox::from_id_salt("redact-pattern").selected_text(tl!(d.pattern.label())).width(crate::widgets::fit_width(ui, 240.0)).show_ui(
            ui,
            |ui| {
                for p in REDACT_PATTERNS {
                    ui.selectable_value(&mut d.pattern, p, tl!(p.label()));
                }
            },
        );
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Text in images isn't found; recognise text (OCR) first.")).small().color(t.text_faint));
    if let Some(n) = d.found {
        ui.label(
            egui::RichText::new(if n == 0 {
                tl!("No matches.").to_string()
            } else {
                crate::i18n::fmt(tl!("{n} match(es) marked for redaction."), &[("n", &n.to_string())])
            })
            .color(t.text_muted),
        );
    }
    ui.add_space(12.0);
    let (go, cancel) = buttons(ui, "Mark all", true);
    (go || enter, cancel, import)
}

/// Redaction Tool Properties. Returns (apply, cancel, open the Redaction Code Editor).
pub(crate) fn props_body(ui: &mut egui::Ui, d: &mut RedactPrefs, _t: &Tokens) -> (bool, bool, bool) {
    let mut edit_codes = false;
    crate::widgets::set_fit_width(ui, 380.0);
    ui.label(egui::RichText::new(tl!("Redaction Tool Properties")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    // On a phone each label sits above its control, and the properties scroll above the buttons.
    let grid = egui::Grid::new("redact-props").num_columns(2).spacing([12.0, 10.0]);
    crate::widgets::dialog_body(ui, "redact-props-body", 64.0, |ui| {
        crate::widgets::form(ui, grid, |ui| {
            ui.label(tl!("Redacted area fill colour:"));
            crate::widgets::row(ui, |ui| {
                let mut none = d.fill.is_none();
                if ui.add(widgets::checkbox(&mut none, tl!("No colour"))).changed() {
                    d.fill = if none { None } else { Some([0.0, 0.0, 0.0]) };
                }
            });
            ui.end_row();
            crate::widgets::form_gap(ui);
            if let Some(c) = crate::comments::swatch_grid(ui, d.fill) {
                d.fill = Some(c);
            }
            ui.end_row();
            crate::widgets::form_gap(ui);
            ui.add(widgets::checkbox(&mut d.use_overlay, tl!("Use overlay text")));
            ui.end_row();
            let on = d.use_overlay;
            let l = ui.add_enabled(on, widgets::radio(!d.use_code, tl!("Custom text:")));
            if l.clicked() {
                d.use_code = false;
            }
            ui.add_enabled(on && !d.use_code, crate::widgets::line(&mut d.overlay).desired_width(crate::widgets::fit_width(ui, 220.0)))
                .labelled_by(l.id);
            ui.end_row();
            if ui.add_enabled(on, widgets::radio(d.use_code, tl!("Redaction code:"))).clicked() {
                d.use_code = true;
            }
            let codes_on = on && d.use_code;
            crate::widgets::row(ui, |ui| {
                ui.add_enabled_ui(codes_on, |ui| {
                    let mut choice: Option<CodeSetChoice> = None;
                    crate::widgets::wrap_before(ui, 150.0);
                    egui::ComboBox::from_id_salt("redaction-code-set").selected_text(d.set_name()).show_ui(ui, |ui| {
                        for set in REDACTION_CODE_SETS {
                            if ui.selectable_label(d.code_set == CodeSetChoice::BuiltIn(set), set.name).clicked() {
                                choice = Some(CodeSetChoice::BuiltIn(set));
                            }
                        }
                        for set in d.code_sets.sets() {
                            let c = CodeSetChoice::Custom(set.name.clone());
                            if ui.selectable_label(d.code_set == c, &set.name).clicked() {
                                choice = Some(c);
                            }
                        }
                    });
                    if let Some(c) = choice {
                        d.choose_set(c);
                    }
                });
                edit_codes = ui.button(tl!("Edit codes…")).clicked();
            });
            ui.end_row();
            let entries = d.set_entries();
            for row in entries.chunks(4) {
                crate::widgets::form_gap(ui);
                ui.add_enabled_ui(codes_on, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (code, description) in row {
                            let mut picked = d.codes.contains(code);
                            let r = if code.chars().count() <= 9 {
                                ui.add_sized([62.0, 18.0], widgets::checkbox(&mut picked, code.as_str()))
                            } else {
                                ui.add(widgets::checkbox(&mut picked, code.as_str()))
                            };
                            let r = if description.is_empty() { r } else { r.on_hover_text(description.as_str()) };
                            if r.changed() {
                                d.codes.retain(|c| c != code);
                                if picked {
                                    d.codes.push(code.clone());
                                }
                            }
                        }
                    });
                });
                ui.end_row();
            }
            let look = &mut d.look;
            ui.label(tl!("Font:"));
            ui.add_enabled_ui(on, |ui| {
                crate::widgets::wrap_before(ui, 150.0);
                egui::ComboBox::from_id_salt("overlay-font").selected_text(look.font.name()).show_ui(ui, |ui| {
                    for f in prckopis_engine::OverlayFont::ALL {
                        ui.selectable_value(&mut look.font, f, f.name());
                    }
                });
            });
            ui.end_row();
            ui.label(tl!("Font size:"));
            ui.add_enabled_ui(on, |ui| {
                crate::widgets::row(ui, |ui| {
                    let mut auto = look.size <= 0.0;
                    if ui.add(widgets::checkbox(&mut auto, tl!("Auto-size text to fit redaction region"))).changed() {
                        look.size = if auto { 0.0 } else { 10.0 };
                    }
                    if !auto {
                        ui.add(egui::DragValue::new(&mut look.size).range(2.0..=144.0).suffix(" pt"));
                    }
                });
            });
            ui.end_row();
            ui.label(tl!("Font colour:"));
            ui.add_enabled_ui(on, |ui| {
                if let Some(c) = crate::comments::swatch_grid(ui, Some(look.color)) {
                    look.color = c;
                }
            });
            ui.end_row();
            crate::widgets::form_gap(ui);
            ui.add_enabled(on, widgets::checkbox(&mut look.repeat, tl!("Repeat overlay text")));
            ui.end_row();
            ui.label(tl!("Text alignment:"));
            ui.add_enabled_ui(on, |ui| {
                crate::widgets::row(ui, |ui| {
                    for (a, label) in [(0u8, tl!("Left")), (1, tl!("Center")), (2, tl!("Right"))] {
                        widgets::radio_value(ui, &mut look.align, a, label);
                    }
                });
            });
            ui.end_row();
        })
    });
    ui.add_space(12.0);
    let (ok, cancel) = buttons(ui, "OK", true);
    (ok, cancel, edit_codes)
}

/// Apply redactions confirmation. Returns (apply, cancel).
pub(crate) fn apply_body(ui: &mut egui::Ui, marks: usize, t: &Tokens) -> (bool, bool) {
    crate::widgets::set_fit_width(ui, 420.0);
    ui.label(egui::RichText::new(tl!("Apply redactions")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(if marks == 1 {
        tl!("You are about to apply 1 redaction mark. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently.").to_string()
    } else {
        crate::i18n::fmt(tl!("You are about to apply {n} redaction marks. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently."), &[("n", &marks.to_string())])
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Save the document afterwards: saving rewrites the whole file so no trace of the removed content stays in it."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(12.0);
    buttons(ui, "Apply", true)
}

fn buttons(ui: &mut egui::Ui, ok: &str, primary: bool) -> (bool, bool) {
    let (mut a, mut c) = (false, false);
    crate::widgets::buttons(ui, |ui| {
        // `ok` stays English internally; only the display is translated.
        if widgets::pill_button(ui, tl!(ok), primary).clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Remove Hidden Information: each category with what was found, checked by default when
/// something was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HiddenDraft {
    pub found: Vec<(prckopis_engine::Hidden, usize, bool)>,
}

impl PrckopisApp {
    pub fn open_remove_hidden(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        self.hidden_draft = HiddenDraft { found: doc.hidden_info().into_iter().map(|(h, n)| (h, n, n > 0)).collect() };
        self.dialog = Some(crate::Dialog::RemoveHidden);
    }
}

/// Returns (remove, cancel).
pub(crate) fn hidden_body(ui: &mut egui::Ui, d: &mut HiddenDraft, t: &Tokens) -> (bool, bool) {
    crate::widgets::set_fit_width(ui, 440.0);
    ui.label(egui::RichText::new(tl!("Remove hidden information")).font(crate::theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Select the items to remove from this document.")).color(t.text_muted));
    ui.add_space(8.0);
    let total: usize = d.found.iter().map(|f| f.1).sum();
    for (h, n, on) in d.found.iter_mut() {
        ui.add_enabled_ui(*n > 0, |ui| {
            crate::widgets::row(ui, |ui| {
                ui.add(widgets::checkbox(on, tl!(h.label())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(if *n == 0 { tl!("None found").to_string() } else { n.to_string() }).color(t.text_muted));
                });
            });
        });
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(tl!("Form fields are flattened: their values stay visible. Saving rewrites the whole file.")).small().color(t.text_faint),
    );
    ui.add_space(12.0);
    let any = d.found.iter().any(|f| f.2 && f.1 > 0);
    let (mut a, mut c) = (false, false);
    crate::widgets::buttons(ui, |ui| {
        if ui.add_enabled_ui(any && total > 0, |ui| widgets::pill_button(ui, tl!("Remove"), true)).inner.clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Sanitize Document confirmation. Returns (sanitize, cancel).
pub(crate) fn sanitize_body(ui: &mut egui::Ui, t: &Tokens) -> (bool, bool) {
    crate::widgets::set_fit_width(ui, 440.0);
    ui.label(egui::RichText::new(tl!("Sanitize document")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(tl!("Sanitizing removes hidden information from the document: metadata, file attachments, comments, form fields (flattened), hidden text and layers, bookmarks, links, actions and scripts, and private application data."));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Save the document afterwards; saving rewrites the whole file so nothing removed stays in it."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(12.0);
    buttons(ui, "Sanitize", true)
}
