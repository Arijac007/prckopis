//! Edit ▸ Search a folder…: search the text of every PDF in a folder (Acrobat's Advanced Search
//! ▸ All PDF documents in), in the background, and open a result at its page with the phrase
//! highlighted.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use pdfcraft_engine::folder_search::{Options, Report};

use crate::PdfCraftApp;
use crate::theme::{self, Tokens};
use crate::widgets;

/// Messages from the search thread.
pub(crate) enum Progress {
    File { done: usize, total: usize, name: String },
    Done(Result<Report, String>),
}

/// The dialog's state, kept between openings (the last search and its results stay).
#[derive(Default)]
pub struct FolderSearch {
    pub query: String,
    pub folder: Option<PathBuf>,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub skip_subfolders: bool,
    pub report: Option<Report>,
    pub error: Option<String>,
    pub(crate) running: Option<(Receiver<Progress>, Arc<AtomicBool>)>,
    pub(crate) progress: (usize, usize, String),
    /// The query and options the shown results are for (opening a result highlights these).
    pub(crate) searched: Option<(String, bool, bool)>,
}

impl FolderSearch {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Start searching in the background (desktop only; the web app has no folders).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(&mut self, ctx: Option<egui::Context>) {
        let Some(folder) = self.folder.clone() else { return };
        if self.query.trim().is_empty() || self.running.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let query = self.query.trim().to_string();
        let opts = Options { case_sensitive: self.case_sensitive, whole_words: self.whole_words, subfolders: !self.skip_subfolders };
        self.searched = Some((query.clone(), opts.case_sensitive, opts.whole_words));
        self.report = None;
        self.error = None;
        self.progress = (0, 0, String::new());
        std::thread::spawn(move || {
            let repaint = |ctx: &Option<egui::Context>| {
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            };
            // A panic in a damaged file's text must not leave the dialog searching forever.
            let result = pdfcraft_engine::guard(|| {
                pdfcraft_engine::folder_search::search_folder(&folder, &query, opts, &stop, |done, total, path| {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    // The receiver may be gone (the app closed): nothing to report to then.
                    let _ = tx.send(Progress::File { done, total, name });
                    repaint(&ctx);
                })
            })
            .unwrap_or_else(|m| Err(format!("an internal error stopped the search ({m})")));
            let _ = tx.send(Progress::Done(result));
            repaint(&ctx);
        });
        self.running = Some((rx, cancel));
    }

    pub fn stop(&mut self) {
        if let Some((_, cancel)) = &self.running {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Take what the search thread sent since the last frame.
    pub(crate) fn poll(&mut self) {
        let Some((rx, _)) = &self.running else { return };
        loop {
            match rx.try_recv() {
                Ok(Progress::File { done, total, name }) => self.progress = (done, total, name),
                Ok(Progress::Done(r)) => {
                    match r {
                        Ok(report) => self.report = Some(report),
                        Err(e) => self.error = Some(e),
                    }
                    self.running = None;
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.running = None;
                    return;
                }
            }
        }
    }
}

/// The dialog. Returns true when it should close.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    app.folder_search.poll();
    let mut close = false;
    let mut choose = false;
    let mut open: Option<(PathBuf, usize)> = None;
    ui.set_width(640.0);
    ui.label(egui::RichText::new(tl!("Search a folder")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let running = app.folder_search.is_running();
    let ctx = app.ctx.clone();
    let s = &mut app.folder_search;
    let l = ui.label(egui::RichText::new(tl!("What word or phrase would you like to search for?")).color(t.text_muted));
    let r = ui.add_enabled(!running, widgets::line(&mut s.query).hint_text(tl!("Search")).desired_width(f32::INFINITY)).labelled_by(l.id);
    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        ui.label(tl!("Look in"));
        if ui.add_enabled(!running, egui::Button::new(tl!("Choose Folder…"))).clicked() {
            choose = true;
        }
        let shown = s.folder.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| tl!("No folder chosen").to_string());
        ui.label(egui::RichText::new(shown).color(t.text_muted));
    });
    ui.add_enabled_ui(!running, |ui| {
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut s.whole_words, tl!("Whole words only")));
            ui.add(widgets::checkbox(&mut s.case_sensitive, tl!("Case-sensitive")));
            let mut subfolders = !s.skip_subfolders;
            if ui.add(widgets::checkbox(&mut subfolders, tl!("Include subfolders"))).changed() {
                s.skip_subfolders = !subfolders;
            }
        });
    });
    ui.add_space(6.0);
    let ready = s.folder.is_some() && !s.query.trim().is_empty();
    ui.horizontal(|ui| {
        if running {
            if widgets::pill_button(ui, tl!("Stop"), false).clicked() {
                s.stop();
            }
            let (done, total, name) = &s.progress;
            ui.spinner();
            ui.label(crate::i18n::fmt(
                tl!("Searching {name} ({d} of {n} files)…"),
                &[("name", name), ("d", &(done + 1).to_string()), ("n", &total.to_string())],
            ));
        } else if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Search"), true)).inner.clicked() || (enter && ready) {
            #[cfg(not(target_arch = "wasm32"))]
            s.start(ctx);
        }
    });
    #[cfg(target_arch = "wasm32")]
    let _ = ctx;
    if let Some(e) = &s.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    if let Some(r) = &s.report {
        ui.add_space(4.0);
        let n = r.hits.len();
        let mut status = if n == 1 {
            crate::i18n::fmt(tl!("1 instance in {f} of {t} files"), &[("f", &r.files_searched.to_string()), ("t", &r.files_found.to_string())])
        } else {
            crate::i18n::fmt(
                tl!("{n} instances in {f} of {t} files"),
                &[("n", &n.to_string()), ("f", &r.files_searched.to_string()), ("t", &r.files_found.to_string())],
            )
        };
        if r.cancelled {
            status.push_str(&format!(" — {}", tl!("stopped")));
        } else if r.truncated {
            status.push_str(&format!(" — {}", tl!("only the first results are shown")));
        }
        ui.label(egui::RichText::new(status).font(theme::semibold(12.5)));
        let folder = s.folder.clone().unwrap_or_default();
        egui::ScrollArea::vertical().max_height(320.0).id_salt("folder-search-results").show(ui, |ui| {
            let mut last: Option<&PathBuf> = None;
            for h in r.hits.iter().take(2000) {
                if last != Some(&h.path) {
                    ui.add_space(4.0);
                    let rel = h.path.strip_prefix(&folder).unwrap_or(&h.path);
                    ui.label(egui::RichText::new(rel.to_string_lossy()).font(theme::semibold(12.5)));
                    last = Some(&h.path);
                }
                let label = format!("{}  {}", crate::i18n::fmt(tl!("Page {label}"), &[("label", &(h.page + 1).to_string())]), h.context);
                if ui
                    .add(egui::Button::new(egui::RichText::new(label).size(12.5)).frame(false).wrap())
                    .on_hover_text(tl!("Open at this page"))
                    .clicked()
                {
                    open = Some((h.path.clone(), h.page));
                }
            }
            if !r.errors.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(tl!("Not searched")).small().color(t.text_faint));
                for (p, e) in &r.errors {
                    let rel = p.strip_prefix(&folder).unwrap_or(p);
                    ui.label(egui::RichText::new(format!("{}: {e}", rel.to_string_lossy())).small().color(t.text_faint));
                }
            }
        });
    }
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("Close"), false).clicked() {
            close = true;
        }
    });
    if close {
        app.folder_search.stop();
    }
    #[cfg(not(target_arch = "wasm32"))]
    if choose {
        let epoch = app.dialog_epoch();
        app.ask_one(crate::pickers::Ask::Folder(rfd::AsyncFileDialog::new()), None, move |app, p| {
            if app.dialog_epoch() == epoch {
                app.folder_search.folder = Some(p);
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = choose;
    if let Some((path, page)) = open {
        app.open_search_result(&path, page);
    }
    close
}

impl PdfCraftApp {
    /// Open a folder-search result (or switch to it when already open) at `page`, with the
    /// searched phrase highlighted as Find does.
    pub(crate) fn open_search_result(&mut self, path: &std::path::Path, page: usize) {
        let wanted = path.to_string_lossy().into_owned();
        let open_tab = self.views.iter().position(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()) == Some(wanted.as_str()));
        match open_tab {
            Some(i) => self.active = Some(i),
            None => {
                let tabs = self.views.len();
                self.open_path(&wanted);
                if self.views.len() == tabs {
                    // Not opened (an error was shown, or a password is being asked for).
                    return;
                }
            }
        }
        let searched = self.folder_search.searched.clone();
        if let Some(i) = self.active
            && let Some(view) = self.views.get_mut(i)
        {
            if let Some((query, case_sensitive, whole_words)) = searched {
                view.find = Some(crate::canvas::Find { query: query.clone(), case_sensitive, whole_words, ..Default::default() });
                view.rerun_find();
            }
            view.go_to_page(page);
        }
    }
}
