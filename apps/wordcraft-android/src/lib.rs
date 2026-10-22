//! WordCraft's Android tablet shell.
//!
//! The Rust engine and egui UI are the same crates the desktop and web builds use. This crate only
//! supplies the Android entry point and hosts [`wordcraft_ui_egui::WordApp`] inside eframe.
//!
//! Android-specific FFI/JNI code is deliberately confined to [`android`], the one module allowed
//! to opt out of the workspace's `unsafe_code = "forbid"` lint. Nothing in that module is used by
//! the engine or UI crates.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(unsafe_code)]

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod android;
