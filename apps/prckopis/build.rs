//! Windows only: embed the app icon and version info (VERSIONINFO) into `prckopis.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab. A separate PDF document icon
//! uses resource ID 2; the installer references it as `prckopis.exe,-2`.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `PRCKOPIS_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

#[path = "src/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/prckopis.ico");
    println!("cargo:rerun-if-changed=../../assets/app-icon/prckopis-document.ico");
    println!("cargo:rerun-if-changed=src/windows_manifest.rs");
    println!("cargo:rerun-if-env-changed=PRCKOPIS_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/prckopis.ico")
        .set_icon_with_id("../../assets/app-icon/prckopis-document.ico", "2")
        .set_manifest(windows_manifest::WINDOWS_MANIFEST)
        .set("ProductName", "Prckopis")
        .set("FileDescription", "Prckopis PDF workbench")
        .set("LegalCopyright", "Copyright (c) the Prckopis contributors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "prckopis.exe")
        .set("InternalName", "prckopis");
    if let Err(e) = res.compile() {
        if std::env::var_os("PRCKOPIS_REQUIRE_WINRES").is_some() {
            println!("cargo::error=embedding Windows resources failed: {e}");
            return;
        }
        println!("cargo:warning=prckopis.exe built without icon/version resources: {e}");
    }
}
