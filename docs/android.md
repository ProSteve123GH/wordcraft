# WordCraft for Android tablets

> **Status:** M-A0 (native shell) · **Last updated:** 2026-10-22 · **Target:** Android 8.0+ (API 26), arm64-v8a

WordCraft's Android build reuses the Rust engine and the egui front end. It is a native APK, not
Termux/X11 and not a WebView wrapper. The tablet is the first-class target; phones come after the
tablet interaction model is stable.

## What M-A0 is

- [x] A `cdylib` entry point (`android_main`) built by `android-activity`.
- [x] AndroidX **GameActivity** as the activity/surface host, with a tiny Java shell.
- [x] eframe 0.36 + wgpu (Vulkan first, GLES fallback where wgpu can use it).
- [x] The same `wordcraft_ui_egui::WordApp` and sample document as the desktop and web builds.
- [x] An arm64 debug-APK CI workflow and a low-RAM local build script.
- [ ] SAF document opening/saving (M-A2).
- [ ] GameActivity IME bridge and composition tests (M-A1).
- [ ] Touch-first ribbon, dialogs and virtual cursor (M-A3/M-A4).

M-A0 deliberately has **no document file access**: it opens the built-in sample, disables AutoSave,
and is a smoke-test target for the native surface, renderer, keyboard and lifecycle. Do not use it
for real work yet.

## Layout

| Path | Purpose |
|---|---|
| `apps/wordcraft-android/src/lib.rs` | Crate root; Android-only module gating |
| `apps/wordcraft-android/src/android.rs` | The one audited host module: `android_main`, logger, eframe adapter |
| `apps/wordcraft-android/android/` | Gradle project, Java `MainActivity`, manifest and theme |
| `packaging/android/build.sh` | Low-RAM local build/install helper |
| `.github/workflows/android.yml` | Off-device full build; uploads the arm64 debug APK |

The Android crate is excluded from the target-specific dependency graph on non-Android hosts, so
ordinary workspace checks do not compile eframe, winit or the NDK. `cargo xtask layers` classifies
it as an app (exempt from layering rules), like the desktop and web binaries.

## Build

The supported full build is GitHub Actions:

```sh
gh workflow run android.yml --ref android/tablet
gh run watch
gh run download <run-id> -n wordcraft-android-arm64-debug
```

It installs Java 21, Android SDK 36, build-tools 36.0.0 and NDK 29.0.14206865, builds
`libwordcraft_android.so` with cargo-ndk, and assembles an arm64 debug APK.

On a tablet with the SDK/NDK installed, use the low-RAM helper:

```sh
export ANDROID_HOME="$HOME/android-sdk"
packaging/android/build.sh --install
```

It sets `CARGO_BUILD_JOBS=1`, disables Cargo incremental artifacts, runs Gradle without a daemon
and caps the Gradle heap at 1 GiB. The `android` Cargo profile keeps optimisation but drops thin-LTO
and debug info. Close other apps first; Android's low-memory killer treats a large Rust link like any
other memory spike.

## Runtime pieces

- **Surface:** GameActivity owns the `SurfaceView` and forwards the native window to winit.
- **Renderer:** eframe's wgpu renderer. `NativeOptions` selects wgpu explicitly; the device's Vulkan
  driver is preferred over the GLES fallback.
- **UI:** `WordApp` draws the existing Word-style ribbon and page canvas. Desktop-only services
  (native pickers, printers, control channel) are not wired yet.
- **Logging:** `android_logger` with the `WordCraft` tag. `adb logcat -s WordCraft` follows startup.
- **Unsafe boundary:** `src/android.rs` is the only module allowed to opt out of `unsafe_code =
  "forbid"`. M-A0's only export is `#[unsafe(no_mangle)] android_main`; future SAF, clipboard and
  IME JNI belongs in the same audited module.

## Next milestones

| Milestone | Outcome |
|---|---|
| M-A1 | GameActivity IME bridge: show/hide the soft keyboard on demand, forward composing text and commits, handle emoji/CJK, hardware-keyboard parity |
| M-A2 | SAF open/save with persistable URI permissions, recovery snapshot before overwrite, byte-based engine I/O |
| M-A3 | Touch-first ribbon, sheets instead of desktop dialogs, page pan/zoom tuned for fingers |
| M-A4 | Trackpad-relative virtual cursor with 1-finger click, 2-finger right-click, hold-drag, 2-finger scroll and pinch zoom |
| M-A5 | Hardware keyboard/mouse shortcuts and focus/selection parity |
| M-A6 | Android clipboard, share/print-as-PDF, intents and recent documents |
| M-A7 | Release signing, Play packaging, crash reporting and update policy |
| M-A8 | Phone layouts and DeX/Chromebook verification |

## Testing notes

- The desktop `ui_shot` harness can inspect the same UI paths offscreen, but Android surface,
  lifecycle, IME and SAF behaviour need an emulator or device.
- M-A0's acceptance test is: the APK installs, launches to the WordCraft sample, renders the ribbon
  and page using Vulkan, accepts hardware-keyboard input, survives rotation/backgrounding, and can
  be captured with `adb exec-out screencap`.
- Real documents and save/reopen are acceptance tests for M-A2, not M-A0.

## Licensing and clean room

No Microsoft iconography, theme assets, fonts or templates are used. The Java shell is original and
the Material/AppCompat theme colours are WordCraft's own. GameActivity and AppCompat are Apache 2.0
dependencies; their licences are recorded by the Android dependency metadata, not copied into
WordCraft's asset tree.
