//! Prckopis on Android: a NativeActivity whose `android_main` runs the same egui app as the
//! desktop build. Build with `cargo apk build -p prckopis-android --release` (docs/android.md).
//!
//! What differs from the desktop: settings, logs and crash recovery live in the app's internal
//! storage (Android has no `HOME`); file dialogs are the app's own browser over shared storage
//! (`prckopis-file-dialog`); logs go to logcat; there is one event loop, so no OpenGL retry; the
//! phone layout is on (`PrckopisApp::compact`); [`Shell`] shows the soft keyboard while a text
//! field has focus and turns the Back button into Escape.

#![cfg_attr(not(target_os = "android"), allow(unused))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

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
