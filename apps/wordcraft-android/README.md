# WordCraft for Android

The real WordCraft engine and egui interface on Android, packaged as a native `NativeActivity`
application: no X11, no Wayland, no Proot and no WebView. The Activity gives winit an
`ANativeWindow`, wgpu presents into it, and the document model, layout, page rasteriser and interface
are the desktop crates unchanged.

This is a **fork addition** (`apps/wordcraft-android/`). Upstream builds for macOS, Windows, Linux,
the BSDs and the web are untouched.

## What Phase 1 contains

- `android_main` entry point, eframe/winit shell, wgpu surface.
- The real `WordApp`: ribbon, rulers, page canvas, panes and status bar.
- A new blank document per launch, system locale/region applied.
- Recovery enabled under `<internal data>/recovery` (an LMK kill does not silently discard work).
- A file log at `<internal data>/wordcraft/logs/android.log`.
- System fonts from `/system/fonts` in addition to the bundled OFL faces.
- A normal window: status bar and navigation/gesture bar stay visible, and the keyboard resizes the
  work area (`adjustResize`). No fullscreen, no immersive mode, no transparent system bars. The shell
  reads the Activity's content rectangle, so the ribbon starts below the status bar and the status
  bar sits above the gesture bar instead of under it.
- Both **arm64-v8a** and **armeabi-v7a** APKs, built by GitHub Actions.

Deliberately **not** in Phase 1 (later phases): the Android file picker and Storage Access Framework,
clipboard bridge, printing, touch gestures/modifier keys, autosave and preference persistence. The
engine and editing commands themselves work; file dialogs and settings are host services.

## Building

Prerequisites (what the CI workflow installs):

- Rust with the `aarch64-linux-android` and/or `armv7-linux-androideabi` target;
- Android SDK with **build-tools 34.0.0**, platform **android-34**, and **NDK r27**;
- JDK 17 or newer (for `apksigner`);
- [`cargo-apk`](https://github.com/rust-mobile/cargo-apk) 0.10.0 (0.9.7 looks for the package
  name rather than the `[lib]` name, so it cannot build a crate whose library is named `wordcraft`).

```bash
export ANDROID_HOME=/path/to/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/27.2.12479018
cargo install cargo-apk --version 0.10.0 --locked

# A throwaway key is enough to sideload a test build.
keytool -genkeypair -v -keystore /tmp/wordcraft-test.jks -alias wordcraft \
        -keyalg RSA -keysize 4096 -validity 10000 \
        -storepass android -keypass android -dname "CN=WordCraft Test, O=WordCraft, C=US"
export CARGO_APK_RELEASE_KEYSTORE=/tmp/wordcraft-test.jks
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android

cargo apk build --release --target aarch64-linux-android -p wordcraft-android
# -> target/release/apk/wordcraft.apk
```

`cargo-apk 0.10.0` drives the legacy `aapt`, so use **build-tools 34.0.0**, not a newer build-tools
release that has dropped it. It also expects NDK r27 (r27 is what the port is tested with).

The APK package id is `ai.storyteller.wordcraft` and the library artifact is `libwordcraft.so`
(`[lib] name = "wordcraft"`); the artifact name is what `android.app.lib_name` points at, so do not
rename it.

## Installing on the tablet

```bash
adb install -r target/release/apk/wordcraft.apk
```

or copy the APK to the tablet and open it (allow installing from this source first).

The CI APK is signed with a throwaway CI key. A build signed with a different key cannot replace an
existing install of the same package id; uninstall the old app first if Android reports a signature
mismatch (`INSTALL_FAILED_UPDATE_INCOMPATIBLE`).

## Phase 1 checks on the device

1. The app opens to the WordCraft Start screen/ribbon on a fresh install.
2. Type text, select it, apply bold/italic, undo and redo.
3. The status bar and the bottom navigation/gesture bar are visible; the document area sits between
   them (no content underneath them, no immersive mode).
4. Rotate the tablet and use split screen: the interface redraws and the document survives.
5. Open the soft keyboard: the work area shrinks instead of the keyboard covering the page.
6. Press Home, reopen the app from Recents: the document is still there.
7. Open a document with a lot of text and scroll/zoom the page.
8. If the app fails to start, retrieve the log without adb:

```bash
adb shell run-as ai.storyteller.wordcraft cat files/wordcraft/logs/android.log
```

or, on a development build only, `adb logcat -s wordcraft`. The log's
`window content … px; system bars … px` line shows the rectangle Android reported; on the tablet it
should have a non-zero top inset (status bar) and a non-zero bottom inset (gesture/button bar).

## Known limitations in Phase 1

- **No file picker / Save As.** `Services::default()` has no native dialog and the Storage Access
  Framework needs `startActivityForResult`, which belongs to Phase 3/4. New documents and editing
  work; opening and saving files do not yet.
- **No clipboard bridge, printing, autosave or persisted preferences.** Recovery exists, but the
  preference file is not written yet.
- **Touch is a pointer.** winit's Android backend does not emit `ModifiersChanged` or wheel events;
  two-finger scrolling, pinch zoom, a modifier-key bridge and a touch-friendly key bar are Phase 3.
- **System bar icons stay dark.** The theme requests dark status/navigation icons; WordCraft's dark
  theme does not restyle the system bars yet (Phase 3).
- **`targetSdkVersion 34`.** This keeps the classic system-bar colours and behaviour. The shell
  already handles the window insets itself, so a future move to 35+ needs no Java `WindowInsets`
  bridge.
- **English-only build unless optional inputs are present.** CI builds without `CRAFT_I18N_DIR` /
  `CRAFT_FONTS_DIR`; the app runs in English with the bundled fonts.
- **No automated device test.** CI proves compilation, packaging and signing only.

See [`../../docs/ANDROID_PORT_PLAN.md`](../../docs/ANDROID_PORT_PLAN.md) for the full design and the
phase split.
