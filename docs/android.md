# Prckopis on Android

> **Status:** first test build (2026-10-13) · **Revision history:** 2026-10-13 created (NativeActivity APK, in-app file browser, release signing)

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

- **Files:** rfd has no Android backend. `crates/file-dialog` (used as `rfd`) is rfd itself
  elsewhere and, on Android, the same API answered by the app's own file browser
  (`crates/ui-egui/src/android_files.rs`) over shared storage (`/storage/emulated/0`). On
  Android 11 and later the browser needs "All files access": Settings ▸ Apps ▸ Prckopis ▸
  Permissions ▸ Files and media ▸ Allow management of all files.
- **Settings, crash recovery:** the app's internal storage (`app.ron`, `recovery/`).
- **Logs:** logcat, tag `Prckopis` (`adb logcat -s Prckopis`).
- **Not available yet:** the clipboard (Create ▸ Clipboard, snapshot to clipboard), screen
  readers (eframe's accesskit needs GameActivity), opening a PDF from another app (VIEW intent),
  system printing (network IPP printers may work), OCR (the models aren't packaged), the
  update check.

## Next

Opening PDFs from other apps and the system picker (Storage Access Framework through JNI); a
compact layout for phones (single panel, larger touch targets, back button, soft keyboard); app
icon; an x86_64 build for emulators; CI.
