# Vendored dependencies for the Termux (Android / X11) port

Termux's Rust target is `aarch64-linux-android`, i.e. `target_os = "android"`. The GUI stack
assumes "unix desktop = Linux", so a few crates are copied here and patched to build and run
their **X11** backend on Android. Everything else is untouched.

| Directory | Version | Patch |
|---|---|---|
| `winit/` | 0.30.13 (storytold fork, `a517b384`) | `build.rs`: `free_unix` no longer excludes Android and `android_platform` is off when the `x11` feature is on (`all(target_os = "android", not(feature = "x11"))`). `Cargo.toml`: the unix/X11/Wayland dependency block also applies to Android. The Android backend (`android-activity`, `ndk`) stays declared because winit's `android-*` features reference it, but it is not compiled into the backend. |
| `egui/` | 0.37.0 (storytold fork, `6d518fc7`) | `eframe`: Android-only code paths (`NativeOptions::android_app`, `with_android_app`, re-create/drop window on suspend) are disabled, so the ordinary desktop event-loop path is used; the event loop also exits as soon as an accepted close has dropped the last window, instead of waiting for a further window event that may never come (the close button could leave the process running with no window). `egui-winit`: the arboard clipboard backend is enabled on Android (X11). |
| `arboard/` | 3.7.0 | The Linux (X11/Wayland) backend is selected on Android instead of "no backend". |
| `rfd/` | 0.17.2 | The Linux (xdg-desktop-portal / zenity) backend is selected on Android. WordCraft uses its own built-in file browser (`WORDCRAFT_FILE_DIALOG=builtin`), so this only keeps the API compiling. |
| `accesskit_winit/` | 0.34.1 | Its default features pull in `winit/wayland`; on Android that drags in `wayland-cursor`, which uses `rustix::shm` (not available for Android in rustix). The `wayland` part of the default features is removed; the X11 and accessibility parts stay. |
| `x11rb-protocol/` | 0.13.2 | Local displays are resolved to a hard-coded `/tmp/.X11-unix/X<n>`, which cannot exist on Android (`/tmp` is not writable). The socket directory is now taken from `X11_UNIX_SOCKET_DIR`, then `$PREFIX/tmp/.X11-unix` (Termux:X11), then `/tmp/.X11-unix`. Without this, arboard's X11 clipboard cannot connect. |

The root `Cargo.toml` points these packages at the copies above through `[patch.crates-io]`.
`vendor/` is excluded from the WordCraft workspace so the vendored egui workspace keeps its own
workspace root.

To refresh a copy: re-copy the crate from `~/.cargo/registry/src` / `~/.cargo/git/checkouts` and
re-apply the edits described in the table (`git diff --no-index` against the original is the
easiest way to review them).
