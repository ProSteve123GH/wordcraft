# WordCraft for Android tablets

> **Status:** M-A2 (SAF storage, Back, safe-area layout, first-run tour, tablet touch) · **Last updated:** 2026-10-23 · **Target:** Android 8.0+ (API 26), arm64-v8a, tablets

WordCraft's Android build reuses the Rust engine and the egui front end. It is a native APK, not
Termux/X11 and not a WebView wrapper. **Tablets are the only supported form factor**: the manifest
requires large/xlarge screens, and the layouts below are designed for 1200×1920 portrait and
1920×1200 landscape. Phone layouts come after the tablet interaction model is stable.

## What works now

- [x] A `cdylib` entry point (`android_main`) built by `android-activity` (M-A0).
- [x] AndroidX **GameActivity** as the activity/surface host, with a tiny Java shell.
- [x] eframe 0.36 + wgpu (Vulkan first, GLES fallback where wgpu can use it).
- [x] The same `wordcraft_ui_egui::WordApp` and sample document as the desktop and web builds.
- [x] The GameActivity IME bridge: show/hide on demand, composing text, commits, Back hides the
      keyboard first (M-A1).
- [x] SAF open/save with persistable URI permissions; Save writes through the picked document,
      Save As and exports use the create-document picker; AutoSave stays off until it can reuse the
      document URI on every path (M-A2).
- [x] Settings in the app's private storage, written a few seconds after a change, when the activity
      pauses, and on quit; a first launch without `ui.json` starts on the welcome tour (M-A2).
- [x] Android Back: keyboard → nearest dialog/page → Save Changes/window close. Unsaved work is
      never discarded (M-A2).
- [x] Real system-bar and display-cutout insets inside `RawInput::safe_area_insets`, so the ribbon,
      page and status bar never sit under the bars or a notch (M-A2).
- [x] First-run welcome tour (Welcome, Storage, Appearance, Editing & Start screen, Done) with
      progress dots, Back/Next, Skip, and a completion flag in `ui.json`; File › Options › General
      shows it again (M-A2).
- [x] Tablet sizing and motion: 44 pt tap targets, a taller ribbon strip and small rows, two-finger
      page pan with coasting, eased pinch zoom, page slide/fade, filling progress dots, the welcome
      mark draw-in, a save pulse and dirty-dot fade (M-A3, in progress).
- [ ] Touch sheets in place of every desktop dialog; on-screen touch handles for tables and objects
      (M-A3).
- [ ] Trackpad-relative virtual cursor with click, hold-drag, two-finger scroll/pinch (M-A4).
- [ ] Clipboard, share, print-as-PDF, intents and recent documents (M-A6).
- [ ] Release signing, Play packaging and crash reporting (M-A7).
- [ ] Phone layouts and DeX/Chromebook verification (M-A8).

## Layout

| Path | Purpose |
|---|---|
| `apps/wordcraft-android/src/lib.rs` | Crate root; Android-only module gating |
| `apps/wordcraft-android/src/android.rs` | The one audited host module: `android_main`, logger, JNI exports, eframe adapter |
| `apps/wordcraft-android/android/` | Gradle project, Java `MainActivity`, manifest, theme, launcher icons |
| `packaging/android/build.sh` | Low-RAM local build/install helper |
| `.github/workflows/android.yml` | Off-device full build; uploads the arm64 debug APK |

The Android crate is excluded from the target-specific dependency graph on non-Android hosts, so
ordinary workspace checks do not compile eframe, winit or the NDK. `cargo xtask layers` classifies
it as an app (exempt from layering rules), like the desktop and web binaries.

## Build

The supported full build is GitHub Actions:

```sh
gh workflow run android.yml --ref android/lifecycle
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
- **Touch:** `Services::touch` is `true`, so the ribbon uses 44 pt targets, `TOUCH_CONTENT_H` and
  `TOUCH_TAB_H`; desktop and web leave it `false` and keep the compact layout. Two fingers pan the
  page 1:1 and coast after they lift; a pinch zooms around the point between the fingers, eased
  towards the fingers when Animation effects are on. One finger stays with the text (caret, selection,
  object drag).
- **Motion:** page transitions, progress dots, the welcome mark, the title's save pulse and the
  dirty dot all route timing through `theme` tokens (`PAGE_SLIDE_SECS`, `DOT_FADE_SECS`, `PULSE_SECS`,
  `MARK_DRAW_SECS`). The Animation effects setting (File › Options › Accessibility) draws their end
  state at once when off. Desktop and web behaviour is unchanged.
- **Safe area:** `MainActivity` reports system bars + display cutout in device-independent pixels;
  Rust divides by the interface zoom and feeds `RawInput::safe_area_insets` every frame. IME
  visibility is reported separately so Back can hide the keyboard before it acts on the app.
- **Back:** API 33+ registers an `OnBackInvokedCallback` (`enableOnBackInvokedCallback="true"`);
  older releases use `onBackPressed`. Both call `nativeBack()`, handled on the next UI pass: the
  keyboard first, then `WordApp::back_gesture` (tour, dialogs, File page, menus, keytips), then the
  host asks `close_requested()` — a dirty document goes through Save Changes.
- **Lifecycle:** `onPause` calls `nativePause()` so settings are flushed even if the activity is
  suspended; `onResume` hides the bars again, requests fresh insets and calls `nativeResume()`. The
  prefs store also saves on `Drop`, so a destroyed activity loses nothing. No lifecycle debug logs.
- **First run:** `PrefsStore::restore` returns whether `ui.json` existed. No file means a first run:
  `ui.onboarding_done` starts `false` and the welcome tour shows. Completing or skipping it writes the
  flag with the other settings; a damaged or oversized `ui.json` still counts as an existing profile
  and never brings the tour back.
- **Logging:** `android_logger` with the `WordCraft` tag. `adb logcat -s WordCraft` follows startup;
  only errors are logged from the Java lifecycle.
- **Unsafe boundary:** `src/android.rs` is the only module allowed to opt out of `unsafe_code =
  "forbid"`. JNI exports (`nativeInsets`, `nativeImeVisible`, `nativeBack`, `nativePause`,
  `nativeResume`, SAF results) sit in that audited module and clamp every number a device or
  activity can send.

## Storage (SAF)

WordCraft asks for no broad storage permission. Android's Files app (the Storage Access Framework)
supplies one document at a time; the permission is persisted for that URI. Open and Save use the
picked document, Save As and exports use a create-document picker, and a failed write leaves the
open document and unsaved changes in place. The tour's Storage page only opens the picker when no
document is open, so it can never replace unsaved work. The SAF save/recents/AutoSave implementation
is owned by `android.rs`; UI code only calls the existing `Services` hooks.

## App icon

`assets/app-icon/wordcraft.svg` is rendered **unmodified** with `rsvg-convert` to
`res/mipmap-{mdpi,hdpi,xhdpi,xxhdpi,xxxhdpi}/ic_launcher.png` at 48, 72, 96, 144 and 192 px. The
manifest sets both `android:icon` and `android:roundIcon` to `@mipmap/ic_launcher`. Every PNG has a
row in `ATTRIBUTION.md`.

## Testing notes

- Off-device, `crates/ui-egui` tests cover the parts that don't need a device:
  - the welcome tour's one-time flag, command, navigation, Skip and reopen, and that its Storage page
    never replaces an open document (`welcome_tour_*`, `the_welcome_tour_*`);
  - Back dismissing the tour and then asking to close through Save Changes
    (`android_back_dismisses_the_welcome_tour_and_never_discards_work`);
  - safe-area insets keeping the page inside the content rect
    (`safe_area_insets_keep_the_page_clear_of_the_gesture_area`);
  - motion, fade and pulse timing with Animation effects on and off, and touch sizing
    (`motion_fades_and_pulses_follow_the_animation_setting`, `touch_style_sizes_for_fingers_and_restores_the_mouse_layout`);
  - two-finger pan/coast, one finger not scrolling, and eased pinch
    (`two_finger_pan_moves_the_page_and_coasts`, `pinch_eases_to_the_fingers_then_finishes`);
  - the title's dirty dot and save pulse (`title_dirty_dot_fades_and_the_save_pulse_runs_once`).
- The desktop `ui_shot` harness can inspect the same UI paths offscreen, but the Android surface,
  lifecycle, IME, SAF and real system bars need an emulator or tablet.
- On-device checklist: install and launch to the welcome tour; finish it and confirm it doesn't
  return after a cold start; rotate between 1200×1920 and 1920×1200 (ribbon, page and status bar stay
  clear of bars/cutout); open a document from the Storage page and via File › Open; type, hide the
  keyboard with Back, then use Back through dialogs to Save Changes; background and resume without
  losing settings; pinch and two-finger pan on a long document; turn Animation effects off and check
  that pages/dots/title are at rest; `adb exec-out screencap` for visual comparison with Word.

## Licensing and clean room

No Microsoft iconography, theme assets, fonts or templates are used. The Java shell is original and
the Material/AppCompat theme colours are WordCraft's own. GameActivity and AppCompat are Apache 2.0
dependencies; their licences are recorded by the Android dependency metadata, not copied into
WordCraft's asset tree. The launcher icon is WordCraft's own SVG, rendered as described above.
