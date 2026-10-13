//! Prckopis on Android: a NativeActivity whose `android_main` runs the same egui app as the
//! desktop build. Build with `cargo apk build -p prckopis-android --release` (docs/android.md).
//!
//! What differs from the desktop: settings, logs and crash recovery live in the app's internal
//! storage (Android has no `HOME`); file dialogs are the app's own browser over shared storage
//! (`prckopis-file-dialog`), which asks Android for file access ([`files`]); logs go to logcat;
//! there is one event loop, so no OpenGL retry; the phone layout is on (`PrckopisApp::compact`);
//! [`Shell`] keeps the app inside the part of the screen the system bars leave free, shows the soft
//! keyboard while a text field has focus and turns the Back button into Escape.

#![cfg_attr(not(target_os = "android"), allow(unused))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
mod files;

#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

/// The app, plus what Android needs around it.
#[cfg(target_os = "android")]
struct Shell {
    app: prckopis_ui_egui::PrckopisApp,
    android: AndroidApp,
    /// Whether the soft keyboard was last asked to show.
    keyboard: bool,
}

/// The space (points) the system bars or a camera cut-out take at the top and bottom of the
/// window: what lies outside Android's content rectangle. Zero when that isn't known yet, and at
/// most a third of the height each, so a bad report can't hide the app.
#[cfg(target_os = "android")]
fn insets(android: &AndroidApp, ctx: &egui::Context) -> (f32, f32) {
    let r = android.content_rect();
    let ppp = ctx.pixels_per_point();
    let height = ctx.content_rect().height();
    if r.bottom <= r.top || !ppp.is_finite() || ppp <= 0.0 {
        return (0.0, 0.0);
    }
    let top = (r.top.max(0) as f32 / ppp).clamp(0.0, height / 3.0);
    let bottom = ((height * ppp - r.bottom as f32) / ppp).clamp(0.0, height / 3.0);
    (top, bottom)
}

#[cfg(target_os = "android")]
impl eframe::App for Shell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Back closes what is open, as Escape does on the desktop (dialogs, panels, menus).
        for event in &mut raw.events {
            if let egui::Event::Key { key, .. } = event
                && *key == egui::Key::BrowserBack
            {
                *key = egui::Key::Escape;
            }
        }
        self.app.raw_input_hook(ctx, raw);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Keep clear of the status bar (and the camera in it) and the navigation bar.
        let (top, bottom) = insets(&self.android, ui.ctx());
        let fill = ui.ctx().global_style().visuals.panel_fill;
        if top > 0.5 {
            egui::Panel::top("android_top_inset").exact_size(top).frame(egui::Frame::NONE.fill(fill)).show(ui, |_| {});
        }
        if bottom > 0.5 {
            egui::Panel::bottom("android_bottom_inset").exact_size(bottom).frame(egui::Frame::NONE.fill(fill)).show(ui, |_| {});
        }
        self.app.ui(ui, frame);
        // NativeActivity shows no keyboard by itself: ask for it while text is being edited.
        let wants = ui.ctx().text_edit_focused();
        if wants != self.keyboard {
            self.keyboard = wants;
            if wants {
                self.android.show_soft_input(true);
            } else {
                self.android.hide_soft_input(true);
            }
        }
    }
}

/// Called by the NativeActivity glue on its own thread when the activity starts.
#[cfg(target_os = "android")]
// The symbol the NativeActivity glue looks up; there is no other way to be called by it.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("Prckopis"));
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let data = app.internal_data_path();
    let android = app.clone();
    // The file browser asks for access the first time a folder can't be read, and from its
    // "Allow access to files" button.
    let for_files = app.clone();
    prckopis_ui_egui::set_file_access_request(move || {
        if let Err(e) = files::request_access(&for_files) {
            log::error!("asking for file access: {e}");
        }
    });
    let options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        persistence_path: data.as_ref().map(|d| d.join("app.ron")),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Prckopis",
        options,
        Box::new(move |cc| {
            let mut app = prckopis_ui_egui::PrckopisApp::new();
            if let Some(json) = cc.storage.and_then(|s| s.get_string("prckopis")) {
                app.restore(&json);
            }
            app.set_compact(true);
            // Autosave unsaved changes; offer to recover what a crashed session left behind.
            if let Some(dir) = &data {
                app.enable_recovery(prckopis_ui_egui::RecoveryStore::new(dir.join("recovery")));
            }
            Ok(Box::new(Shell { app, android, keyboard: false }))
        }),
    );
    if let Err(e) = result {
        log::error!("Prckopis could not start: {e}");
    }
}
