# WordCraft for Android

The real WordCraft engine and egui interface on Android, packaged as a native `NativeActivity`
application: no X11, no Wayland, no Proot and no WebView. The Activity gives winit an
`ANativeWindow`, wgpu presents into it, and the document model, layout, page rasteriser and interface
are the desktop crates unchanged.

This is a **fork addition** (`apps/wordcraft-android/`). Upstream builds for macOS, Windows, Linux,
the BSDs and the web are untouched.

## What the port contains (Phases 1–3)

- `android_main` entry point, eframe/winit shell, wgpu surface.
- **Touch is a touchpad, not a finger-sized mouse.** One finger moves a visible pointer, a tap
  clicks, a second tap and drag selects text, two fingers scroll/pinch or tap to right-click,
  and three fingers tap middle-click or swipe down to show/hide the extra key bar. Four or more
  fingers are ignored until all lift, and a cancelled gesture never leaves a button down.
- **Physical devices are detected and logged.** Java's `InputManager` reports when a mouse, touchpad or
  alphabetic keyboard is attached or removed (`input devices: …`). A physical keyboard's typing and
  shortcut keys arrive through winit; the shell tracks the left/right modifier key events because
  winit's Android backend sends no modifier state. Mouse hover, wheel and extra buttons are discarded
  by winit's Android backend, so a physical mouse cannot yet drive the page pointer (see the
  limitations).
- **Extra key bar:** Ctrl, Alt, Shift (sticky), Esc, Tab, arrows, Home, End, Page Up and Page Down,
  drawn above the status bar and resized by the keyboard.
- **Back and IME:** with the keyboard visible, Back hides it; with a text editor still focused but
  the keyboard hidden, Back brings it back; elsewhere Back follows Android's normal behaviour.
- **Window insets from `WindowInsets`, not only the content rectangle.** Java reports status-bar,
  cutout and navigation/gesture-bar insets; the shell uses them as egui's safe area and falls back to
  the old content rectangle only before Java has reported. Both system bars stay visible.
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
  work area (`adjustResize`). No fullscreen, no immersive mode, no transparent system bars.
- Both **arm64-v8a** and **armeabi-v7a** APKs, built by GitHub Actions.

Deliberately **not** in Phases 1–3 (Phase 4): the Android file picker and Storage Access Framework,
clipboard bridge, printing, autosave and preference persistence. The engine and editing commands
themselves work; file dialogs and settings are host services.

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
APK="$(find target -name wordcraft.apk -path '*/apk/*' -print -quit)"

# cargo-apk builds only Rust. Compile the Java MainActivity, dex it into the APK and sign again,
# otherwise the APK names an Activity that is not in its classes.dex and will not launch.
apps/wordcraft-android/package-java.sh "$APK"
```

`cargo-apk 0.10.0` drives the legacy `aapt`, so use **build-tools 34.0.0**, not a newer build-tools
release that has dropped it. It also expects NDK r27 (r27 is what the port is tested with). The
packaging script needs `javac`, `zip`/`unzip`, and the same build-tools 34.0.0 and platform 34.

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

## Touch, mouse, keyboard and the key bar

Touch is translated by `src/touchpad.rs` before egui sees it, so the document UI stays a desktop
word processor rather than growing a second, finger-sized interface:

| Gesture | Result |
|---|---|
| One finger drag | Move the visible pointer. It starts where the finger first touches and then moves relative to it, like a touchpad. |
| One finger tap | Left click at the pointer. Two taps close together are a double click; a third is a triple click. |
| Tap, then drag (or long press, then drag) | Press the left button and drag: text selection, object move, scrollbar drag. The press happens only after movement, so a tap never starts a selection by accident. |
| Two finger drag | Scroll the document under the fingers; it never selects. The canvas keeps its normal momentum coast. |
| Two finger pinch/spread | Zoom the page around the fingers. |
| Two finger tap | Right click at the pointer (context menu). |
| Three finger tap | Middle click at the pointer (autoscroll). |
| Three finger swipe down | Show or hide the extra key bar. |
| Four or more fingers | Ignored; every finger must lift before a new gesture starts. |

Android reports one finger of a two-finger move at a time. A scroll therefore briefly looks like a
pinch while only the first finger has moved, so the recogniser waits for both fingers (or ~120 ms)
before deciding. `src/touchpad.rs` is pure Rust with host tests for all of the above, including
cancel/ghost-click suppression and hostile coordinates.

A `NativeActivity` takes the window's input queue, so Java `dispatchTouchEvent`/`dispatchKeyEvent`
overrides never see pointer or key events; the device listener is the Java part that works. Device
attach/remove is logged as `input devices: mouse=… touchpad=… keyboard=…`. Physical keyboard events
arrive through winit, but winit's Android backend sends no `ModifiersChanged`; the shell tracks
`ShiftLeft`/`ShiftRight`, `ControlLeft`/`ControlRight`, `AltLeft`/`AltRight` and the Super keys as
they are pressed and released. Without this, Ctrl/Alt/Shift shortcuts would never fire from a
hardware keyboard. A physical mouse/trackpad's hover, wheel and extra buttons are dropped by winit
before the app sees them (see the limitations).

The key bar is drawn in the Android shell, not in the shared UI crate. Ctrl/Alt/Shift are sticky:
tap one, then an arrow, Home/End/Page Up/Page Down, Esc or Tab (or use a physical keyboard key). The
bar is inside the safe area, so the keyboard resizing the window moves it up rather than covering it.
Three-finger swipe down hides it to give the document more height.

### Back and the IME

`MainActivity` handles Back through `onBackPressed` and also registers an `OnBackInvokedCallback` on
Android 13+, so it works whether or not predictive back is enabled for the target SDK. Both ask the
native side whether a text editor is focused:

- keyboard visible → hide it, stay in the app;
- keyboard hidden but a text editor focused → show it again;
- no text-editing focus → normal Android Back (leave the Activity).

Show/hide use `AndroidApp::show_soft_input`/`hide_soft_input` (native, routed through Java) rather
than `InputMethodManager.showSoftInput` on the decor view, because a `NativeActivity` window is not
an `EditText` the IME can attach to. The window keeps `SOFT_INPUT_ADJUST_RESIZE`, and the IME inset
is part of the safe area as well, so opening the keyboard shrinks the document area and the key bar
instead of covering them.

### System bars

Both system bars stay visible. `MainActivity` installs an `OnApplyWindowInsetsListener` and reports
`systemBars() | displayCutout() | ime()` in physical pixels. The shell divides by the window density
and sets egui's `safe_area_insets` and its own layout rectangle from them. Including the IME matters
because NativeActivity gives the surface the whole window: on Android 11+ the window does not
reliably shrink for the keyboard, so the shell has to make room for it itself. This is the primary
source; the `AndroidApp::content_rect` arithmetic in `src/insets.rs` remains as a fallback until Java
has reported real insets. It is also what makes the layout correct on Android 15's forced
edge-to-edge, where the content rectangle can be the whole window.

## Installing on the tablet

```bash
adb install -r target/release/apk/wordcraft.apk
```

or copy the APK to the tablet and open it (allow installing from this source first).

The CI APK is signed with a throwaway CI key. A build signed with a different key cannot replace an
existing install of the same package id; uninstall the old app first if Android reports a signature
mismatch (`INSTALL_FAILED_UPDATE_INCOMPATIBLE`).

## Device checks (Phases 1–3)

1. The app opens to the WordCraft Start screen/ribbon on a fresh install.
2. Type text, select it, apply bold/italic, undo and redo.
3. The status bar and the bottom navigation/gesture bar are visible; the ribbon starts below the
   status bar and the status bar sits above the gesture bar (no content underneath either, no
   immersive mode). The log has a `window insets … px; safe area … pt` line with non-zero top and
   bottom insets.
4. One finger moves the visible pointer. A tap clicks a ribbon button; tap-drag selects text; a
   two-finger drag scrolls without selecting; pinch zooms; two-finger tap opens the context menu;
   three-finger tap middle-clicks; three-finger swipe down hides and shows the key bar.
5. Tap Ctrl in the key bar and then →: the caret moves a word. Shift + → selects. Tap Ctrl again to
   clear it. Esc, Tab, Home, End, PgUp and PgDn do what they say.
6. Open the soft keyboard: the work area and the key bar shrink instead of being covered. Press Back:
   the keyboard hides, the document stays. Press Back again: the keyboard returns. With no editor
   focused, Back leaves the app normally.
7. Connect a USB/Bluetooth mouse: the log has `input devices: mouse=true`; Android's hardware cursor
   moves over the app. Connect a physical keyboard and try Ctrl+B, Ctrl+Z and Shift+arrows (the
   modifier tracking above). Mouse clicks may arrive as touch and act like a finger tap, but hover,
   wheel and right-click are not yet forwarded (see limitations).
8. Rotate the tablet and use split screen: the interface redraws inside the new insets and the
   document survives.
9. Press Home, reopen the app from Recents: the document is still there.
10. The log has a `wgpu ready:` line with `backend Vulkan` (preferred) or `backend Gl` (fallback), and
    no `software rasteriser in use` warning.
11. Run the frame-time benchmark above; it logs a finite `cpu ms` and `fps` summary and the interface
    stays responsive afterwards.
12. If the app fails to start, retrieve the log without adb:

```bash
adb shell run-as ai.storyteller.wordcraft cat files/wordcraft/logs/android.log
```

or, on a development build only, `adb logcat -s wordcraft`. The log's
`window insets … px; safe area … pt` line (or the older `window content … px; system bars … px`
fallback) shows what Android reported; on the tablet it should have a non-zero top inset (status
bar) and a non-zero bottom inset (gesture/button bar).

## Known limitations in Phases 1–3

- **No file picker / Save As.** `Services::default()` has no native dialog and the Storage Access
  Framework needs `startActivityForResult`, which belongs to Phase 4. New documents and editing
  work; opening and saving files do not yet.
- **No clipboard bridge, printing, autosave or persisted preferences.** Recovery exists, but the
  preference file is not written yet.
- **Sticky modifiers and soft-keyboard letters.** Android IMEs usually commit letters as text rather
  than key events, so Ctrl + a letter typed on the soft keyboard may insert the letter instead of
  running the shortcut. Ctrl/Alt/Shift work with the key bar's navigation keys and with a physical
  keyboard. The ribbon's Copy/Paste/Undo buttons cover the common soft-keyboard cases.
- **Touchpad mode is deliberate.** A finger is a relative pointer, not a direct touch target: tapping
  a button means moving the pointer there first. This keeps WordCraft's desktop UI (small ribbon
  buttons, text selection, drag handles) usable without a second, touch-only interface. Direct
  touch-to-position can be added later if the owner wants it.
- **No stylus pressure/tilt, handwriting or palm rejection beyond Android's own.** A stylus is passed
  to winit as touch; it goes through the same touchpad model.
- **Physical mouse buttons, hover and wheel are not forwarded.** winit's Android backend only
  translates motion events with touch actions and key events; it discards hover, scroll and
  `ACTION_BUTTON_PRESS/RELEASE` (its Android source marks mouse support `TODO`). A `NativeActivity`
  also takes the window input queue, so Java cannot intercept them. Full mouse/trackpad support needs
  a winit (or android-activity) patch; until then a physical mouse is detected and its left click may
  arrive as a touch, but it cannot hover, scroll or right-click. The touchpad model covers the
  tablet's own screen and the extra key bar covers keyboard shortcuts.
- **System bar icons stay dark.** The theme requests dark status/navigation icons; WordCraft's dark
  theme does not restyle the system bars yet.
- **`targetSdkVersion 34`.** This keeps the classic system-bar colours and behaviour. The Java
  `WindowInsets` bridge is already in place, so moving to 35+ no longer requires new inset code.
- **English-only build unless optional inputs are present.** CI builds without `CRAFT_I18N_DIR` /
  `CRAFT_FONTS_DIR`; the app runs in English with the bundled fonts.
- **No automated device test.** CI proves compilation, Java dexing, packaging, signing, the launchable
  Activity, the JNI exports and the host-tested gesture model. Back/IME, physical devices, insets and
  the GPU adapter are verified manually on the tablet; the benchmark measures CPU time and frame
  pacing, not per-draw GPU timestamps.
- **No runtime backend switch.** The backend is chosen at startup and logged. Switching from Vulkan
  to GLES requires a relaunch with `WORDFCRAFT_GPU` (or a device with no usable Vulkan adapter);
  there is no in-app settings toggle yet.

See [`../../docs/ANDROID_PORT_PLAN.md`](../../docs/ANDROID_PORT_PLAN.md) for the full design and the
phase split.
