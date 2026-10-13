# Prckopis on Android

> **Status:** test builds, phone layout (2026-10-13) · **Revision history:** 2026-10-13 created (NativeActivity APK, in-app file browser, release signing); 2026-10-13 phone layout, soft keyboard, Back; 2026-10-13 system bars and camera cut-out, file access

`apps/prckopis-android` runs the same egui app as the desktop build inside an Android
NativeActivity (`android_main`). All of it is Rust: no Java, Kotlin or Gradle.

## Build

Needs the Android SDK (build-tools, a platform), the NDK (r27c is known to work), the Rust target
and cargo-apk:

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk --locked
export ANDROID_HOME=<sdk>  ANDROID_NDK_ROOT=<sdk>/android-ndk-r27c
cargo apk build -p prckopis-android --release
```

The APK lands in `<target dir>/release/apk/prckopis.apk` (arm64-v8a; Android 8.0, API 26, and
later).

### Release signing

Release APKs are signed with the owner's key (certificate `CN=Ari Matahari, O=Prckopis`). The
keystore and its password are **never** in the repository; cargo-apk reads them from the
environment:

```sh
export CARGO_APK_RELEASE_KEYSTORE=<path>/ari-matahari-release.jks
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD=<password>
```

Every version must be signed with the same key, or Android refuses to install it over the
previous one. Back the keystore up: a lost key means users have to uninstall to update.
Check a build with `apksigner verify --print-certs prckopis.apk`.

## What differs from the desktop

- **Phone layout** (`PrckopisApp::compact`, below 840 points wide; a tablet in landscape keeps
  the desktop layout): one top bar (Menu, All tools, the open document — tap to switch, go Home
  or close — Find tools and commands, Open, Save), page navigation and zoom at the bottom with
  the side panels behind its panel button, the tools and side panels as a resizable sheet over
  the lower part of the page, a 10-point gutter so the page fills the width, and 40-point touch
  targets. Back closes the sheet (and dialogs, as Escape does). `--compact on` shows it on the
  desktop for screenshots (`cargo run -p prckopis-ui-egui --example shot -- out.png doc.pdf
  --size 400x860 --compact on`); `crates/ui-egui/tests/compact.rs` tests it.
- **Soft keyboard:** shown while a text field has focus, hidden after.
- **Files:** rfd has no Android backend. `crates/file-dialog` (used as `rfd`) is rfd itself
  elsewhere and, on Android, the same API answered by the app's own file browser
  (`crates/ui-egui/src/android_files.rs`) over shared storage (`/storage/emulated/0`). On
  Android 11 and later that needs "All files access" (without it Android lists only the app's own
  files, so PDFs are missing without an error): the first file dialog opens Android's All files
  access screen for Prckopis (`apps/prckopis-android/src/files.rs`, through JNI), the browser has
  an "Allow access to files" button, and an empty folder is read again every second so the files
  appear once access is granted. Android 10 and earlier get the storage permission prompt.
- **System bars:** the activity uses `Theme.DeviceDefault.NoActionBar` (not full screen), so the
  app sits below the status bar and the camera cut-out and above the navigation bar; the shell
  also keeps clear of whatever Android's content rectangle excludes.
- **Settings, crash recovery:** the app's internal storage (`app.ron`, `recovery/`).
- **Logs:** logcat, tag `Prckopis` (`adb logcat -s Prckopis`).
- **Not available yet:** the clipboard (Create ▸ Clipboard, snapshot to clipboard), screen
  readers (eframe's accesskit needs GameActivity), opening a PDF from another app (VIEW intent),
  system printing (network IPP printers may work), OCR (the models aren't packaged), the
  update check.

## Next

Opening PDFs from other apps and the system picker (Storage Access Framework through JNI);
long-press menus; dialogs that fit a phone (several use fixed 300–380 point widths); app icon; an
x86_64 build for emulators; CI.
