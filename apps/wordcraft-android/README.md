# WordCraft for Android

The real WordCraft engine and egui interface on Android, packaged as a native `NativeActivity`
application: no X11, no Wayland, no Proot and no WebView. The Activity gives winit an
`ANativeWindow`, wgpu presents into it, and the document model, layout, page rasteriser and interface
are the desktop crates unchanged.

This is a **fork addition** (`apps/wordcraft-android/`). Upstream builds for macOS, Windows, Linux,
the BSDs and the web are untouched.

## What the port contains (Phases 1–2)

- `android_main` entry point, eframe/winit shell, wgpu surface.
- **GPU selection:** wgpu is asked for both **Vulkan** and **OpenGL ES** adapters, a hardware Vulkan
  adapter is preferred, hardware OpenGL ES is the fallback, and a software rasteriser is refused
  unless a diagnostic variable says otherwise. The requested backends, every candidate adapter
  (name, backend, device type, driver), the selected adapter and the render target are written to
  the log. GLES device requests are clamped to the driver's texture limit instead of assuming 8k.
- **Performance benchmark:** an opt-in frame-time run (see below) logs CPU time and frame pacing
  for the real interface; normal use stays reactive and does not repaint in a loop.
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

## GPU backend and performance

The renderer is wgpu. On Android the shell configures it explicitly instead of relying on wgpu's
platform defaults:

- `wgpu::Backends::VULKAN | wgpu::Backends::GL` are requested. Adapter enumeration therefore covers
  devices with a working Vulkan driver *and* devices where only `libEGL`/GLES is usable.
- A custom adapter selector prefers hardware Vulkan, then hardware OpenGL ES. It skips adapters that
  cannot present to the Activity surface. If only a software rasteriser remains, startup fails with
  a clear log line instead of silently running at single-digit FPS.
- Software rendering can be forced for diagnostics with an environment variable (emulators/CI):
  `WORDFCRAFT_GPU=software` or `WORDFCRAFT_ALLOW_SOFTWARE_GPU=1`. `WORDFCRAFT_GPU=vulkan|gles` and
  `WGPU_BACKEND` narrow the backends; the normal setting is `auto`.
- The device request uses the lower of 8k and the adapter's maximum texture size, and `downlevel` GL
  limits for the GLES backend, so a device request does not fail on a 4k-max GLES driver.
- The surface uses egui's low-latency configuration (vsync-paced, one queued frame). Repaints remain
  reactive; only the benchmark (and normal interaction) requests frames.

The selected renderer is logged twice, once by the selector and once from the renderer eframe
actually created:

```text
wgpu: requesting Backends(VULKAN | GL); software adapters allowed: false
wgpu adapter candidate: Adreno (TM) 610 (backend Vulkan, device IntegratedGpu, driver "Adreno")
wgpu selected: Adreno (TM) 610 (backend Vulkan, device IntegratedGpu, driver "Adreno")
wgpu ready: Adreno (TM) 610 (backend Vulkan, device IntegratedGpu), target Bgra8Unorm, 2 adapter(s) enumerated
```

Read it with:

```bash
adb shell run-as ai.storyteller.wordcraft cat files/wordcraft/logs/android.log | grep wgpu
```

### Frame-time benchmark

The benchmark is opt-in. Either set `WORDFCRAFT_BENCH_FRAMES=600` in the process environment
(emulator/CI) or write a small file in the app's internal data directory and relaunch:

```bash
adb shell "run-as ai.storyteller.wordcraft sh -c 'echo frames=600 > files/bench.conf'"
adb shell am force-stop ai.storyteller.wordcraft
adb shell monkey -p ai.storyteller.wordcraft 1
adb shell run-as ai.storyteller.wordcraft cat files/wordcraft/logs/android.log | grep bench
```

Optional file lines: `warmup=60` (default 30, capped at half the measured frames) and comments
starting with `#`. The shell repaints continuously for `frames + warmup` frames and writes one
summary:

```text
bench: 600 frames (60 warm-up): cpu ms min=2.10 avg=4.32 p50=4.05 p95=6.20 max=19.80; frame ms min=15.90 avg=16.68 p95=17.02 -> 60.0 fps
```

`cpu` is eframe's per-frame CPU time (interface, layout, page rasterisation and wgpu submission,
excluding the vsync wait); `frame` is the wall-clock interval between frames and includes vsync
pacing, so `fps` reflects the display. The run keeps the real interface on screen. Remove
`files/bench.conf` (or unset the variable) to return to normal reactive repainting. GPU frame time
needs an external profiler (e.g. Android GPU Inspector or `adb shell dumpsys gfxinfo`); the app
logs the backend and adapter needed to interpret those traces.

## Installing on the tablet

```bash
adb install -r target/release/apk/wordcraft.apk
```

or copy the APK to the tablet and open it (allow installing from this source first).

The CI APK is signed with a throwaway CI key. A build signed with a different key cannot replace an
existing install of the same package id; uninstall the old app first if Android reports a signature
mismatch (`INSTALL_FAILED_UPDATE_INCOMPATIBLE`).

## Device checks (Phases 1–2)

1. The app opens to the WordCraft Start screen/ribbon on a fresh install.
2. Type text, select it, apply bold/italic, undo and redo.
3. The status bar and the bottom navigation/gesture bar are visible; the document area sits between
   them (no content underneath them, no immersive mode).
4. Rotate the tablet and use split screen: the interface redraws and the document survives.
5. Open the soft keyboard: the work area shrinks instead of the keyboard covering the page.
6. Press Home, reopen the app from Recents: the document is still there.
7. Open a document with a lot of text and scroll/zoom the page.
8. The log has a `wgpu ready:` line with `backend Vulkan` (preferred) or `backend Gl` (fallback), and
   no `software rasteriser in use` warning.
9. Run the frame-time benchmark above; it logs a finite `cpu ms` and `fps` summary and the interface
   stays responsive afterwards.
10. If the app fails to start, retrieve the log without adb:

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
- **No automated device test.** CI proves compilation, packaging and signing only. The GPU adapter
  choice, pause/resume and the frame-time benchmark are verified manually on the tablet; the
  benchmark measures CPU time and frame pacing, not per-draw GPU timestamps.
- **No runtime backend switch.** The backend is chosen at startup and logged. Switching from Vulkan
  to GLES requires a relaunch with `WORDFCRAFT_GPU` (or a device with no usable Vulkan adapter);
  there is no in-app settings toggle yet.

See [`../../docs/ANDROID_PORT_PLAN.md`](../../docs/ANDROID_PORT_PLAN.md) for the full design and the
phase split.
