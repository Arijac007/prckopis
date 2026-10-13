//! Android's file dialogs: rfd has no Android backend, so `rfd` there is
//! `prckopis-file-dialog`'s shim, which queues each dialog for the app to answer. This draws them as
//! an in-app file browser over shared storage (`/storage/emulated/0`), one at a time.
//!
//! Reading shared storage needs Android's "All files access" for the app. Without it, Android 11
//! and later list only the app's own files (no error: the PDFs are simply missing). The host asks
//! for it ([`set_file_access_request`]): once when the browser first opens, and from its "Allow
//! access to files" button; a folder that showed nothing is read again every second, so the files
//! appear once access is granted.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};

type AccessRequest = Box<dyn Fn() + Send + Sync>;

fn access_request() -> &'static OnceLock<AccessRequest> {
    static REQUEST: OnceLock<AccessRequest> = OnceLock::new();
    &REQUEST
}

/// How the host asks Android for file access (Settings ▸ All files access, or the storage
/// permissions on older versions). Set once, before the first file dialog.
pub fn set_file_access_request(f: impl Fn() + Send + Sync + 'static) {
    let _ = access_request().set(Box::new(f));
}

fn ask_for_access() {
    if let Some(f) = access_request().get() {
        f();
    }
}

use rfd::host::{Kind, Request};

/// Shared storage, where Downloads, Documents and the user's files are.
const STORAGE: &str = "/storage/emulated/0";
/// The most entries a folder lists (its folders first).
const MAX_ENTRIES: usize = 5_000;

struct Browser {
    request: Request,
    dir: PathBuf,
    /// The file name typed when saving.
    name: String,
    /// The files picked so far (Files picks several).
    picked: Vec<PathBuf>,
    /// The folder's folders, then the files the dialog accepts; or why it couldn't be read.
    listing: Result<Vec<(String, bool)>, String>,
}

impl Browser {
    fn new(request: Request) -> Self {
        // The first time in a session, ask for access (nothing happens when it is already there).
        static ASKED: Once = Once::new();
        ASKED.call_once(ask_for_access);
        let start = request
            .directory
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| LAST_DIR.with(|d| d.borrow().clone()))
            .unwrap_or_else(|| Path::new(STORAGE).join("Download"));
        let name = request.file_name.clone().unwrap_or_default();
        let mut b = Browser { request, dir: start.clone(), name, picked: Vec::new(), listing: Ok(Vec::new()) };
        b.go(start);
        b
    }

    fn go(&mut self, dir: PathBuf) {
        self.listing = list(&dir, &self.request);
        self.dir = dir;
    }
}

/// The entries of `dir` the dialog shows: folders (true), then the files its filters accept.
fn list(dir: &Path, request: &Request) -> Result<Vec<(String, bool)>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut out: Vec<(String, bool)> = entries
        .filter_map(Result::ok)
        .take(MAX_ENTRIES)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (is_dir || (request.kind != Kind::Folder && request.accepts(&name))).then_some((name, is_dir))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    Ok(out)
}

thread_local! {
    static CURRENT: RefCell<Option<Browser>> = const { RefCell::new(None) };
    static LAST_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Draw the file dialog waiting for an answer, if any. Call once per frame.
pub(crate) fn show(ctx: &egui::Context) {
    static NOTIFY: Once = Once::new();
    NOTIFY.call_once(|| {
        let ctx = ctx.clone();
        rfd::host::set_notify(move || ctx.request_repaint());
    });
    CURRENT.with(|cell| {
        let mut current = cell.borrow_mut();
        if current.is_none() {
            *current = rfd::host::next().map(Browser::new);
        }
        let Some(b) = current.as_mut() else { return };
        // A folder that couldn't be read, or showed nothing to pick, may fill once access is
        // granted: read it again every second while the browser is open.
        let empty = match &b.listing {
            Err(_) => true,
            Ok(entries) => b.request.kind != Kind::Folder && entries.iter().all(|(_, is_dir)| *is_dir),
        };
        if empty {
            let now = ctx.input(|i| i.time);
            let due = ctx.data(|d| d.get_temp::<f64>(egui::Id::new("android-files-reread"))).is_none_or(|t| now - t >= 1.0);
            if due {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("android-files-reread"), now));
                let dir = b.dir.clone();
                b.go(dir);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        let mut answer: Option<Vec<PathBuf>> = None;
        let title = b.request.title.clone().unwrap_or_else(|| {
            match b.request.kind {
                Kind::File | Kind::Files => tl!("Open"),
                Kind::Folder => tl!("Choose this folder"),
                Kind::Save => tl!("Save"),
            }
            .to_owned()
        });
        let screen = ctx.content_rect();
        crate::widgets::show_modal(ctx, "android-file-browser", |ui| {
            ui.set_width((screen.width() - 48.0).clamp(240.0, 720.0));
            ui.heading(&title);
            ui.horizontal_wrapped(|ui| {
                if ui.button(tl!("Internal storage")).clicked() {
                    b.go(PathBuf::from(STORAGE));
                }
                if ui.button(tl!("Downloads")).clicked() {
                    b.go(Path::new(STORAGE).join("Download"));
                }
                if let Some(parent) = b.dir.parent().map(Path::to_path_buf)
                    && ui.button(tl!("Up")).clicked()
                {
                    b.go(parent);
                }
            });
            ui.label(egui::RichText::new(b.dir.to_string_lossy()).small());
            if empty && ui.button(tl!("Allow access to files")).clicked() {
                ask_for_access();
            }
            ui.separator();
            let mut go_to = None;
            egui::ScrollArea::vertical().max_height((screen.height() - 260.0).max(160.0)).show(ui, |ui| match &b.listing {
                Err(e) => {
                    ui.label(crate::i18n::fmt(
                        tl!("This folder can't be read ({e}). Allow Prckopis to access files in Android Settings ▸ Apps ▸ Prckopis ▸ Permissions."),
                        &[("e", e)],
                    ));
                }
                Ok(entries) => {
                    for (name, is_dir) in entries {
                        let path = b.dir.join(name);
                        let label = if *is_dir { format!("📁 {name}") } else { name.clone() };
                        let chosen = b.picked.contains(&path);
                        let row = ui.add_sized([ui.available_width(), 40.0], egui::Button::selectable(chosen, label));
                        if row.clicked() {
                            if *is_dir {
                                go_to = Some(path);
                            } else {
                                match b.request.kind {
                                    Kind::File => answer = Some(vec![path]),
                                    Kind::Files if chosen => b.picked.retain(|p| p != &path),
                                    Kind::Files => b.picked.push(path),
                                    Kind::Save => b.name = name.clone(),
                                    Kind::Folder => {}
                                }
                            }
                        }
                    }
                }
            });
            if let Some(dir) = go_to {
                b.go(dir);
            }
            if b.request.kind == Kind::Save {
                ui.horizontal(|ui| {
                    ui.label(tl!("File name"));
                    ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(f32::INFINITY));
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tl!("Cancel")).clicked() {
                    answer = Some(Vec::new());
                }
                let (label, ready) = match b.request.kind {
                    Kind::File => (None, false),
                    Kind::Files => (Some(tl!("Open")), !b.picked.is_empty()),
                    Kind::Folder => (Some(tl!("Choose this folder")), b.listing.is_ok()),
                    Kind::Save => (Some(tl!("Save")), !b.name.trim().is_empty() && !b.name.contains('/')),
                };
                if let Some(label) = label
                    && ui.add_enabled(ready, egui::Button::new(label)).clicked()
                {
                    answer = Some(match b.request.kind {
                        Kind::Files => b.picked.clone(),
                        Kind::Folder => vec![b.dir.clone()],
                        _ => vec![b.dir.join(b.name.trim())],
                    });
                }
            });
        });
        if let Some(paths) = answer
            && let Some(done) = current.take()
        {
            LAST_DIR.with(|d| *d.borrow_mut() = Some(done.dir.clone()));
            done.request.answer(paths);
            ctx.request_repaint();
        }
    });
}
