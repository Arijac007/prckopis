//! Prckopis's file dialogs, used by `prckopis-ui-egui` under the name `rfd`.
//!
//! Where [rfd](https://docs.rs/rfd) has a backend (desktop, the web) this is rfd itself. On
//! Android, which it lacks, it is [`shim`]: the same API, answered by the app's own file browser.
//! The shim builds everywhere, so its tests run on any machine.

#[cfg(not(target_os = "android"))]
pub use rfd::*;

pub mod shim;

#[cfg(target_os = "android")]
pub use shim::*;
