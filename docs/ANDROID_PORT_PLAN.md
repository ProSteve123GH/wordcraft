# WordCraft on Android — port plan

**Status:** Phase 1 implemented; both APK ABIs build and verify in CI. The first device run showed the
ribbon under the status bar; the shell now applies the Activity's content rectangle (see §3) and the
fix is awaiting re-test on the tablet.
**This is a fork addition.** Upstream WordCraft targets macOS, Windows, Linux, the BSDs and the web;
the Android port lives in `apps/wordcraft-android/` and keeps the desktop, web and CLI builds intact.

## 1. What the workspace actually contains (inspected 2026-11-06)

| Item | Finding |
|---|---|
| Version / commit | WordCraft **0.15.1**, commit **`247c7fea`** (fork `ProSteve123GH/wordcraft`, branch `main`, up to date with the checkout) |
| Language / toolchain | Rust **1.95**, edition **2024**, workspace lints forbid `unsafe_code` |
| Window / UI | `eframe` **0.37** + `egui` 0.37 from **`storytold/egui`** @ `6d518fc7`; the desktop shell implements eframe's `logic`/`ui`/`raw_input_hook` hooks |
| Windowing | `winit` **0.30.13** from **`storytold/winit`** @ `a517b384` (a fork of upstream 0.30.13) |
| Graphics | `wgpu` **30.0.1** through `eframe`'s `wgpu` feature and `egui-wgpu`; document pages are rasterised on the CPU by `vello_cpu` and painted as egui textures |
| Entry point | `apps/wordcraft/src/main.rs`: `eframe::run_native("WordCraft", …)`; `App` implements `eframe::App` |
| Settings | `wordcraft_control_key::settings_dir()` (`HOME`/XDG/`%APPDATA%`); `ui.json` written atomically in `save_prefs`, called from `on_exit`; recovery is an explicit `Session::enable_recovery(root, wake)` |
| File picker | `rfd` native dialogs on the desktop via `Services::file_dialog` (async, non-blocking); `Services::pick_open`/`pick_save` are synchronous fallbacks; a built-in `file_browser` exists for hosts without a system dialog |
| Existing Android code | **None.** The only Android-aware code is defensive `cfg` exclusions in the desktop clipboard and the Unix path. `Cargo.lock` already contains `android-activity 0.6.1` (winit's target dependency) and `ndk` |
| Existing Android CI | **None.** `.github/workflows/` has desktop/web/release jobs only |

### Android capability already present in the pinned dependencies

- eframe 0.37 (the storytold fork) still has `android-native-activity` and `android-game-activity`
  features and `NativeOptions::android_app`. `eframe::run_native` builds the winit event loop with
  `EventLoopBuilderExtAndroid::with_android_app`.
- `android-native-activity` + `accesskit` is a deliberate `compile_error!` in eframe: the AccessKit
  Android adapter only supports GameActivity. The Android shell will not enable `accesskit` on eframe.
- The winit fork keeps its Android backend (`src/platform_impl/android/`). It is upstream 0.30.13 behaviour:
  it forwards lifecycle (`Resumed`/`Suspended`), touch and key events, but **not** `ModifiersChanged`,
  mouse-wheel/motion events or insets (`MainEvent::InsetsChanged` is a logged TODO). It refuses
  `set_fullscreen` on Android ("Cannot set fullscreen on Android"), so system bars are the theme's
  business. `android-activity` still stores the content rectangle from `onContentRectChanged`
  (`AndroidApp::content_rect`); winit ignores the event, but the shell reads the stored value.
- eframe treats Android as non-desktop (`IS_DESKTOP` is false), so `ctx.embed_viewports()` is true.
  WordCraft's `windows_ui` then draws the active window in the root viewport instead of creating a
  second native window — exactly what a single-Activity Android app needs.
- The pinned winit uses `android-activity 0.6`; the `android_main(app: AndroidApp)` entry point is a
  re-export at `winit::platform::android::activity::AndroidApp`.

## 2. Reference port: `kanaaka/wordcraft-android`

That community port targets **upstream 0.5.0** and publishes an **arm64-v8a** APK. Its
`docs/PORT-NOTES.md` is a fix log, not a patch we can apply. What transfers, and what does not:

| Reference finding | Applies here? | Our use |
|---|---|---|
| Shell crate in `apps/`, `[lib] name = "wordcraft"`, `crate-type = ["cdylib"]`, `NativeOptions::android_app = Some(app)` | Yes | Same shape; the artifact name is what cargo-apk points `android.app.lib_name` at |
| Omit `[lints] workspace = true` because the entry point needs `#[unsafe(no_mangle)]` and the workspace forbids `unsafe_code` | Yes | Same; keep the rest of the crate panic-free by hand |
| `config_changes = "orientation\|keyboardHidden\|screenSize\|smallestScreenSize\|screenLayout\|density\|uiMode\|fontScale\|keyboard\|navigation\|touchscreen"` | Yes | Prevents NativeActivity recreation, which destroys the surface while the loop lives on (blank screen) |
| Egui file picker on `Services::file_dialog`; preferences written from the frame loop; modifier bridge; two-finger scroll; vendored winit for wheel events | Later phases | Phase 3 (input) and Phase 4 (files/settings/autosave). Phase 1 deliberately keeps `Services::default()` |
| Immersive fullscreen theme (their variant B) | **No** | The user explicitly requires status bar + navigation/gesture bar to stay visible |
| Target SDK 35 + edge-to-edge workarounds | **No for Phase 1** | See §3 below |
| ARM64 only | **No** | We build arm64-v8a **and** armeabi-v7a |

## 3. Phase 1 architecture

```
Android NativeActivity (no Java/Kotlin code)
        │  android_main(app)
        ▼
eframe 0.37 ── winit 0.30.13 Android backend (ANativeWindow surface, Resumed/Suspended)
        │
        ▼
wgpu 30 surface (Vulkan / GLES chosen by wgpu)
        │
        ▼
egui 0.37 (WordCraft ribbon, page canvas, status bar)
        │
        ▼
wordcraft-engine / doc / layout / render (vello_cpu page rasterisation)
```

- **New crate** `apps/wordcraft-android`, package `wordcraft-android`, library **`wordcraft`**
  (`cdylib`). It depends on `wordcraft-engine`, `wordcraft-ui-egui`, `wordcraft-fonts`, `log`,
  `egui`, `eframe` (no `accesskit`, `android-native-activity`, `wgpu`) and `winit` (Android backend).
  No desktop shell crate is reused: `apps/wordcraft` pulls `rfd`, `arboard`, `craft-print`,
  X11/Wayland/zbus, all of which are desktop-only.
- **No Java, no Gradle**: `cargo-apk 0.10.0` assembles the manifest, resources, native library and
  signs the APK with a CI-generated throwaway key. This keeps the local and CI footprint minimal.
  (0.9.7 cannot be used: it derives the `.so` name from the package name, not the `[lib]` name.)
- **Window / insets**: `targetSdkVersion = 34`, no `FLAG_FULLSCREEN`, no immersive mode, no
  `windowFullscreen`, no transparent system bars. The surface winit receives is the **whole** window,
  including the status bar and navigation/gesture bar area, and Android does not inset the
  `ANativeWindow` itself (the first tablet run drew the ribbon under the status bar). `NativeActivity`
  does lay its content view out between the bars and reports the result through
  `onContentRectChanged`; `android-activity 0.6` stores that in `AndroidApp::content_rect`, while
  winit 0.30 drops the event. The shell therefore reads the rectangle every frame and
  (`src/insets.rs`) turns it into egui's `RawInput::safe_area_insets`, then lays the interface out in
  `Context::content_rect` — `viewport_rect` minus the system bars. This needs no Java `WindowInsets`
  bridge and also keeps dialogs/tooltips out of the bars. The rectangle is polled for a short while
  at startup and after a surface resize, because winit does not wake the loop for the change.
  `NativeActivity` sets `SOFT_INPUT_ADJUST_RESIZE` itself, so the keyboard resizes the work area.
- **Lifecycle**: `configChanges` keeps the Activity alive across rotation, split screen, IME and
  font-scale changes; winit/eframe recreate the wgpu surface on `Resumed` and pause rendering on
  `Suspended`. The document/session lives in `WordApp`, outside the surface, so pause/resume does not
  lose editing state. Recovery is enabled against the app's internal data folder so an LMK kill does
  not silently discard work (full autosave is Phase 4).
- **Fonts**: `FontDb::system_font_dirs` gains `/system/fonts` for Android, so installed Noto faces are
  discoverable; the bundled OFL faces still guarantee text without them.
- **Logging**: a small `log` file logger writes to `<internal data>/wordcraft/logs/android.log`
  (logcat is unavailable to a sideloaded build without adb). No new dependency.
- **Settings**: not persisted in Phase 1 (`settings_dir()` has no Android answer yet); Phase 4 adds a
  proper `internal_data_path`-based settings store and the frame-loop atomic writer. Recovery is
  enabled now because it is cheap and protects the document.

## 4. Dependencies and architecture risks (checked before coding)

- **Desktop-only crates are not in the Android graph**: `rfd`, `arboard`, `craft-print`, `zbus`,
  `wl-clipboard-rs`, `x11rb`, `fmv-macos-events`, `objc2*`, `windows`, `open`, `clipboard-win`.
- **In the Android graph and expected to cross-compile**:
  - `sys-locale 0.3.2` has an `android` provider module (verified in its source);
  - `craft-proof`, `vello_cpu`, `skrifa`, `harfrust`, `image`, `zip`, `krilla`/`craft-pdf` are pure Rust;
  - `craft-font-store`/`craft-update` bring `ureq` + `rustls` + `ring`; ring 0.17 has Android arm64
    and armv7 pregenerated code. `rustls-platform-verifier` has a JNI Android backend; its Java
    verifier is only touched by update/font-store downloads, which Phase 1 never starts.
  - `getrandom`, `libc`, `rayon`, `chrono(clock)` support Android; `chrono` falls back to UTC when no
    zone database exists (acceptable for Phase 1; date-field work is engine-level).
- **ARM32**: the only credible blockers are native crypto (`ring`) and wgpu's GLES path. Both are
  expected to work on `armv7-linux-androideabi`; the CI matrix builds both ABIs from the first run.
  If ARM32 fails, the exact crate/error goes in the report and stays a requirement until solved.
- **`unsafe_code`**: the cdylib entry point needs `#[unsafe(no_mangle)]`. The crate therefore does not
  inherit workspace lints (the same documented choice as the reference port); no `unsafe` blocks are
  written and the crate still denies the panic lints itself.
- **Lockfile**: adding a workspace member requires its `Cargo.lock` entry; it is added in the same
  commit so `--locked` builds and `cargo xtask layers` stay green.

## 5. CI plan (GitHub Actions, fork only)

Workflow `.github/workflows/android.yml`:

1. **`check`** (ubuntu, lightweight): `cargo test -p wordcraft-android` (host-testable helpers) and
   `cargo clippy -p wordcraft-android --all-targets -- -D warnings`; assets and layers checks run as
   a separate small step (`cargo xtask assets`, `cargo xtask layers`) so attribution and layering are
   enforced in CI even though the build is cross-only.
2. **`apk`** matrix `arm64-v8a` → `armeabi-v7a` with `max-parallel: 1` (sequential, per the brief):
   - Rust stable + both targets; `Swatinem/rust-cache` with `cache-targets: false` (Cargo registry
     and git only; no multi-GB target caches, well under the 10 GB budget);
   - `cargo-apk 0.10.0` binary cached separately (≈5 MB);
   - JDK 21 (apksigner) and NDK r27 from the runner image, installed with `sdkmanager` only if missing;
   - throwaway keystore generated in `$RUNNER_TEMP` (never committed), same store/key password;
   - `cargo apk build --release --target <triple> -p wordcraft-android`;
   - verify with `aapt2 dump badging` (package, SDK levels, `native-code`) and
     `apksigner verify --verbose --print-certs`; fail if the ABI is wrong;
   - upload the APK as `wordcraft-android-<abi>` and the build log on failure.

No Android SDK/NDK or APK is committed. No signing secret is committed; a CI-only debug-grade key
signs test APKs. Release signing is a later decision for the owner.

## 6. Phase 1 scope and acceptance

**In scope:** the crate, Android entry point, theme/manifest/resources, `/system/fonts`, logging,
recovery hook-up, the workflow, the lockfile/layering registration, this document, README with
install/test steps.

**Out of scope (later phases):** GPU backend selection/logging and page-pipeline profiling (2); the
Termux:X11-style touchpad, gesture recogniser, extra key bar and Back-to-open-IME (3); SAF/file
browser default folder, DOCX filtering, PDF export checks, autosave, settings persistence, cursor
performance (4).

**Acceptance:** APK builds for both ABIs in CI; a signed APK is attached; manifest shows
`minSdk 26`, `targetSdk 34`, both ABIs, normal theme (no fullscreen flags); the app launches the real
WordCraft interface on the tablet; pausing/resuming and rotating does not blank or crash.
Device verification is the owner's, on the Samsung Galaxy Tab A11+; CI can only prove compilation,
packaging and signing.

## 7. Open decisions deferred to the owner / later phases

- Package id `ai.storyteller.wordcraft` is kept for continuity; a sideloaded APK signed by a different
  key cannot replace the community port's install without uninstalling it first. Changing the id is a
  one-line decision if both should coexist.
- Whether to move to `targetSdk 35+` (the shell already handles insets; only the Play-console
  requirements and system-bar styling remain) and to Play-style release signing.

## 8. Phase 1 implementation

These files implement the plan above:

| Path | What it does |
|---|---|
| `apps/wordcraft-android/Cargo.toml` | New crate (`cdylib` named `wordcraft`), cargo-apk manifest, SDK 26/34, both ABIs, full `configChanges`, no workspace lints (the entry point needs `#[unsafe(no_mangle)]`) |
| `apps/wordcraft-android/src/lib.rs` | `android_main(app)` entry point, panic lints |
| `apps/wordcraft-android/src/app.rs` | eframe shell, session, system region, user fonts, recovery, `WordApp`, safe-area insets |
| `apps/wordcraft-android/src/insets.rs` | Content-rectangle → system-bar insets arithmetic, host-tested |
| `apps/wordcraft-android/src/logging.rs` | Dependency-free file logger with host tests |
| `apps/wordcraft-android/res/values/styles.xml` | Visible light system bars, no fullscreen/immersive |
| `apps/wordcraft-android/res/mipmap-xxxhdpi/ic_launcher.png` | Launcher icon (copy of the original 512 px app icon, row in `ATTRIBUTION.md`) |
| `apps/wordcraft-android/README.md` | Build, install and device checklist |
| `crates/fonts/src/fontdb/names.rs` | `/system/fonts` on Android |
| `xtask/src/layers.rs` | Registers the `android` app crate as exempt |
| `.github/workflows/android.yml` | Host checks + sequential arm64/armv7 APK builds, verification and artifacts |

The workflow installs **build-tools 34.0.0** and **NDK r27** (cargo-apk 0.10.0 drives legacy `aapt`,
which newer build-tools no longer ship) and exposes a private SDK view containing only build-tools
34 so ndk-build does not select a newer one. APKs are signed with a throwaway CI key generated in
`$RUNNER_TEMP`; nothing is committed.

**CI evidence (2026-11-06):** [Android run 42244008938](https://github.com/ProSteve123GH/wordcraft/actions/runs/42244008938)
built both APKs. `aapt2 dump badging` reported package `ai.storyteller.wordcraft`, `sdkVersion 26`,
`targetSdkVersion 34`, label `WordCraft` and the expected `native-code` for each ABI;
`apksigner verify` accepted v2/v3 signatures. The upstream [CI gate 42244009696](https://github.com/ProSteve123GH/wordcraft/actions/runs/42244009696)
also passed, so the host workspace, formatting, clippy, tests, assets and layering checks stay green.

## Revision history

| Date | Change |
|---|---|
| 2026-11-06 | Phase 0 inspection and plan written. |
| 2026-11-06 | Phase 1 implementation: crate, shell, theme, logging, fonts, layering, workflow and README added. cargo-apk pinned to 0.10.0 so the `[lib] name = "wordcraft"` artifact is found. |
| 2026-11-06 | CI: both ABIs built, badging and v2/v3 signatures verified; upstream CI gate green. Device verification remains. |
| 2026-11-07 | First device run: ribbon drawn under the status bar. Root cause: `ANativeWindow` is the whole window, not the content area, and winit ignores `ContentRectChanged`. Fix: read `AndroidApp::content_rect`, set egui's safe area and lay out inside `Context::content_rect`; host tests for the inset arithmetic; theme comment corrected (`NativeActivity` sets `adjustResize`). |
