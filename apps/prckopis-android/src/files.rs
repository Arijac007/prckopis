//! Asking Android for access to shared storage, for the app's file browser.
//!
//! Android 11 (API 30) and later: "All files access" (`MANAGE_EXTERNAL_STORAGE`) is a special
//! permission the user grants on a Settings screen; this opens that screen for the app when it
//! isn't granted yet. Android 10 and earlier: the runtime storage permissions, requested from the
//! activity. Nothing happens when access is already there.

use jni::JavaVM;
use jni::objects::{JObject, JValue};
use winit::platform::android::activity::AndroidApp;

/// Ask for access to shared storage, if the app doesn't have it.
pub(crate) fn request_access(app: &AndroidApp) -> jni::errors::Result<()> {
    // SAFETY: android-activity hands out the process's JavaVM pointer, valid for the life of the
    // process; `from_raw` only wraps it.
    #[allow(unsafe_code)]
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    // SAFETY: the activity pointer is a global reference android-activity keeps alive while the
    // app runs; the wrapper borrows it and never deletes it.
    #[allow(unsafe_code)]
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
    if sdk >= 30 {
        let granted = env.call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])?.z()?;
        if granted {
            return Ok(());
        }
        let package = env.call_method(&activity, "getPackageName", "()Ljava/lang/String;", &[])?.l()?;
        let prefix = env.new_string("package:")?;
        let uri_text = env.call_method(&prefix, "concat", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&package)])?.l()?;
        let uri = env.call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&uri_text)])?.l()?;
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let intent =
            env.new_object("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[JValue::Object(&action), JValue::Object(&uri)])?;
        if env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)]).is_err() {
            // Some devices lack the per-app screen: open the list of apps with that access.
            let _ = env.exception_clear();
            let action = env.new_string("android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION")?;
            let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
            env.call_method(&activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&intent)])?;
        }
    } else if sdk >= 23 {
        let names = ["android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE"];
        let string = env.find_class("java/lang/String")?;
        let empty = env.new_string("")?;
        let array = env.new_object_array(names.len() as i32, &string, &empty)?;
        for (i, name) in names.iter().enumerate() {
            let s = env.new_string(name)?;
            env.set_object_array_element(&array, i as i32, &s)?;
        }
        env.call_method(&activity, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&array), JValue::Int(1)])?;
    }
    Ok(())
}
