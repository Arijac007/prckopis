//! The part of [rfd](https://docs.rs/rfd)'s API that Prckopis uses, for Android, where rfd has
//! no backend.
//!
//! A dialog here shows nothing itself. Awaiting [`AsyncFileDialog::pick_file`] (and the others)
//! queues a [`host::Request`] and blocks the awaiting thread until the app answers it: the app
//! draws its own file browser for [`host::next`] and calls [`host::Request::answer`]. The UI
//! awaits pickers on a worker thread, never on the frame's thread, so the block is harmless.

use std::future::Future;
use std::path::{Path, PathBuf};

/// A file dialog being set up: what it asks for, shown by the app's browser.
#[derive(Clone, Debug, Default)]
pub struct AsyncFileDialog {
    title: Option<String>,
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
    directory: Option<PathBuf>,
}

impl AsyncFileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Show only files with these extensions (without the dot), under `name`.
    pub fn add_filter(mut self, name: impl Into<String>, extensions: &[impl ToString]) -> Self {
        self.filters.push((name.into(), extensions.iter().map(ToString::to_string).collect()));
        self
    }

    /// The suggested name of a file to save.
    pub fn set_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// The folder to start in.
    pub fn set_directory(mut self, path: impl AsRef<Path>) -> Self {
        self.directory = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn pick_file(self) -> impl Future<Output = Option<FileHandle>> {
        let paths = host::ask(self, host::Kind::File);
        async move { paths.wait().into_iter().next().map(FileHandle) }
    }

    pub fn pick_files(self) -> impl Future<Output = Option<Vec<FileHandle>>> {
        let paths = host::ask(self, host::Kind::Files);
        async move {
            let picked: Vec<FileHandle> = paths.wait().into_iter().map(FileHandle).collect();
            (!picked.is_empty()).then_some(picked)
        }
    }

    pub fn pick_folder(self) -> impl Future<Output = Option<FileHandle>> {
        let paths = host::ask(self, host::Kind::Folder);
        async move { paths.wait().into_iter().next().map(FileHandle) }
    }

    pub fn save_file(self) -> impl Future<Output = Option<FileHandle>> {
        let paths = host::ask(self, host::Kind::Save);
        async move { paths.wait().into_iter().next().map(FileHandle) }
    }
}

/// A chosen file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHandle(PathBuf);

impl FileHandle {
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file_name(&self) -> String {
        self.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The file's bytes; empty when it can't be read.
    pub async fn read(&self) -> Vec<u8> {
        std::fs::read(&self.0).unwrap_or_default()
    }
}

/// The app's side: the dialogs waiting for an answer.
pub mod host {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Mutex, OnceLock};

    use super::AsyncFileDialog;

    /// What a dialog asks for.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Kind {
        /// One existing file.
        File,
        /// One or more existing files.
        Files,
        /// A folder.
        Folder,
        /// A file name to save to (it may exist).
        Save,
    }

    /// A dialog waiting for the app to answer.
    #[derive(Debug)]
    pub struct Request {
        pub kind: Kind,
        pub title: Option<String>,
        /// (name, extensions without the dot); empty: every file.
        pub filters: Vec<(String, Vec<String>)>,
        /// The suggested name when saving.
        pub file_name: Option<String>,
        /// The folder to start in, if the caller named one.
        pub directory: Option<PathBuf>,
        reply: Sender<Vec<PathBuf>>,
    }

    impl Request {
        /// Answer the dialog: the chosen paths, or none when cancelled.
        pub fn answer(self, paths: Vec<PathBuf>) {
            // The asker may have gone (its thread ended); then there is no one to tell.
            let _ = self.reply.send(paths);
        }

        /// Whether `name` passes the dialog's filters (always, without filters).
        pub fn accepts(&self, name: &str) -> bool {
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
            self.filters.is_empty()
                || self.filters.iter().any(|(_, exts)| {
                    exts.iter().any(|e| e == "*" || ext.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(e.trim_start_matches('.'))))
                })
        }
    }

    type Notify = Box<dyn Fn() + Send + Sync>;

    fn queue() -> &'static Mutex<VecDeque<Request>> {
        static QUEUE: OnceLock<Mutex<VecDeque<Request>>> = OnceLock::new();
        QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
    }

    fn notify() -> &'static Mutex<Option<Notify>> {
        static NOTIFY: OnceLock<Mutex<Option<Notify>>> = OnceLock::new();
        NOTIFY.get_or_init(|| Mutex::new(None))
    }

    /// Called whenever a dialog is queued, so the app can draw it (e.g. request a repaint).
    pub fn set_notify(f: impl Fn() + Send + Sync + 'static) {
        // A poisoned lock only means a panic elsewhere while it was held; the slot is still usable.
        let mut slot = notify().lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(Box::new(f));
    }

    /// The next dialog waiting for an answer, if any.
    pub fn next() -> Option<Request> {
        queue().lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }

    /// The answer a queued dialog is waiting for.
    pub(crate) struct Pending(Receiver<Vec<PathBuf>>);

    impl Pending {
        /// Block until the app answers; cancelled (empty) if it drops the request unanswered.
        pub(crate) fn wait(self) -> Vec<PathBuf> {
            self.0.recv().unwrap_or_default()
        }
    }

    pub(crate) fn ask(d: AsyncFileDialog, kind: Kind) -> Pending {
        let (reply, answer) = channel();
        let request = Request { kind, title: d.title, filters: d.filters, file_name: d.file_name, directory: d.directory, reply };
        queue().lock().unwrap_or_else(|e| e.into_inner()).push_back(request);
        if let Some(f) = notify().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            f();
        }
        Pending(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(f: impl Future<Output = T>) -> T {
        // A tiny executor: the futures here never return Pending.
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn a_pick_waits_for_the_app_and_cancelling_answers_none() {
        let asker = std::thread::spawn(|| block(AsyncFileDialog::new().set_title("Open").add_filter("PDF", &["pdf"]).pick_file()));
        let request = loop {
            if let Some(r) = host::next() {
                break r;
            }
            std::thread::yield_now();
        };
        assert_eq!((request.kind, request.title.as_deref()), (host::Kind::File, Some("Open")));
        assert!(request.accepts("a.PDF") && !request.accepts("a.png") && !request.accepts("pdf"));
        request.answer(vec![PathBuf::from("/sdcard/a.pdf")]);
        assert_eq!(asker.join().unwrap().map(|h| h.file_name()), Some("a.pdf".to_string()));

        let asker = std::thread::spawn(|| block(AsyncFileDialog::new().pick_files()));
        let request = loop {
            if let Some(r) = host::next() {
                break r;
            }
            std::thread::yield_now();
        };
        assert!(request.accepts("anything"));
        drop(request);
        assert_eq!(asker.join().unwrap(), None, "an unanswered dialog is cancelled");
    }
}
