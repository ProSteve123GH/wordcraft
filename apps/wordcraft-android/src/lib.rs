//! WordCraft for Android.
//!
//! The real engine and the real egui interface, packaged as a `NativeActivity` application: no X11,
//! no Wayland, no Proot and no WebView. The Android Activity gives winit an `ANativeWindow`; wgpu
//! presents into it; the interface, document model, layout and page rasteriser are the desktop
//! crates unchanged.
//!
//! The entry point is `android_main`, called by `android-activity` on its own thread. The Activity
//! is declared with the full `configChanges` set in `Cargo.toml` so rotation, split screen and the
//! keyboard resize the window instead of recreating the Activity (which would destroy the surface
//! under the running render loop).
//!
//! Phase 1 keeps the desktop shell's `Services` out: file pickers, clipboard bridges, printing and
//! settings persistence are Phase 3/4 work. The document engine, editing, layout and rendering are
//! all live.
//!
//! The status bar and the navigation/gesture bar stay visible: the shell reads the Activity's
//! content rectangle and lays the interface out only inside it (see `insets`).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod logging;

/// The part of the window surface the system bars leave to the app. Compiled on the host too, so
/// the arithmetic has unit tests.
#[cfg(any(target_os = "android", test))]
mod insets;

/// wgpu backend/adapter policy. The pure selection is host-tested; the wgpu plumbing is Android
/// only, like the rest of the shell.
#[cfg(any(target_os = "android", test))]
mod gpu;

/// Opt-in on-device frame-time benchmark; host-tested statistics.
#[cfg(any(target_os = "android", test))]
mod bench;

#[cfg(target_os = "android")]
mod app;

#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

/// The Android application entry point, called by `android-activity`'s `NativeActivity` glue.
///
/// `unsafe(no_mangle)` is required because the Java side looks the symbol up by name; it is the
/// only `unsafe` attribute in the crate and the crate does not perform any unsafe operation.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    app::run(app);
}
