//! Prckopis on Android: a NativeActivity whose `android_main` runs the same egui app as the
//! desktop build. Build with `cargo apk build -p prckopis-android --release` (docs/android.md).
//!
//! What differs from the desktop: settings, logs and crash recovery live in the app's internal
//! storage (Android has no `HOME`); file dialogs are the app's own browser over shared storage
//! (`prckopis-file-dialog`); logs go to logcat; there is one event loop, so no OpenGL retry.

#![cfg_attr(not(target_os = "android"), allow(unused))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

/// Called by the NativeActivity glue on its own thread when the activity starts.
#[cfg(target_os = "android")]
// The symbol the NativeActivity glue looks up; there is no other way to be called by it.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("Prckopis"));
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    let data = app.internal_data_path();
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
            // Autosave unsaved changes; offer to recover what a crashed session left behind.
            if let Some(dir) = &data {
                app.enable_recovery(prckopis_ui_egui::RecoveryStore::new(dir.join("recovery")));
            }
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = result {
        log::error!("Prckopis could not start: {e}");
    }
}
