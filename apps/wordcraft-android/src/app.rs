//! The Android application shell: build the session, wrap `WordApp` in eframe's `App` trait and run
//! the event loop against the Activity's native window.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use winit::platform::android::activity::AndroidApp;

use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

use crate::{bench, gpu, insets, logging};

const APP_NAME: &str = "WordCraft";

/// Sets up logging, fonts and recovery, then runs eframe until the Activity ends.
pub(crate) fn run(android_app: AndroidApp) {
    let data_dir = android_app.internal_data_path();
    let log_path = logging::init(data_dir.as_deref());
    log::info!("WordCraft {} starting on Android", env!("CARGO_PKG_VERSION"));
    if let Some(path) = &log_path {
        log::info!("log file {}", path.display());
    }

    // User fonts live beside the other private data; a missing folder is normal on first run.
    if let Some(dir) = data_dir.as_deref() {
        let (loaded, skipped) = wordcraft_fonts::FontDb::global().load_user_fonts(&dir.join("fonts"));
        if !loaded.is_empty() || !skipped.is_empty() {
            log::info!("user fonts: {} loaded, {} skipped", loaded.len(), skipped.len());
        }
    }

    // Keep a handle for the shell: `ANativeWindow` is the whole surface, including the status and
    // navigation bars, so the shell asks the Java side where the content view actually is (see
    // `Shell::raw_input_hook`). `AndroidApp` is a cheap `Arc` clone.
    let insets_app = android_app.clone();
    let mut options = eframe::NativeOptions {
        android_app: Some(android_app),
        viewport: egui::ViewportBuilder::default().with_title(APP_NAME).with_app_id("ai.storyteller.wordcraft"),
        ..Default::default()
    };
    // Vulkan first, OpenGL ES as the fallback; the adapter is chosen at runtime and logged. See
    // `gpu` for why the backends are requested explicitly rather than left to wgpu's defaults.
    gpu::configure(&mut options);

    let bench_config = bench::BenchConfig::load(data_dir.as_deref());
    if let Some(config) = bench_config {
        log::info!("benchmark: {} frames after {} warm-up frames (WORDFCRAFT_BENCH_FRAMES)", config.frames, config.warmup);
    }

    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            // The selector already logged its choice; this is the renderer eframe actually made,
            // including the software-rasteriser warning and the target format.
            gpu::log_ready(&cc);
            // The system region picks the new document's paper, the units and the decimal separator,
            // as the desktop shell does.
            let region = wordcraft_ui_egui::i18n::system_region();
            log::info!("system region: {}-{}", region.language, region.territory);
            let document = wordcraft_engine::blank_document(&region);
            let mut session = Session::new(document);
            session.system_region = region;
            let mut app = WordApp::new(session, Services::default());
            // Recovery is cheap and protects an unsaved document from an LMK kill. Autosave itself
            // (writing the original file) is Phase 4.
            if let Some(dir) = data_dir.as_ref() {
                let ctx = cc.egui_ctx.clone();
                if let Err(e) = app.session.enable_recovery(dir.join("recovery"), Arc::new(move || ctx.request_repaint())) {
                    log::warn!("local recovery unavailable: {e}");
                }
            }
            Ok(Box::new(Shell {
                app,
                android_app: insets_app,
                content: None,
                window_px: (0, 0),
                settle: 3,
                bench: bench_config.map(bench::FrameBench::new),
            }))
        }),
    );

    if let Err(e) = result {
        log::error!("eframe event loop ended with an error: {e}");
    }
    log::info!("WordCraft stopped");
    logging::flush();
}

/// eframe's app for the Android Activity.
struct Shell {
    app: WordApp,
    /// The Activity, for asking where the system bars are. `AndroidApp` is a cheap `Arc` clone.
    android_app: AndroidApp,
    /// The content rectangle the insets were last computed from, so a change is logged once.
    content: Option<insets::ContentRect>,
    /// The surface size in physical pixels, to notice rotation and keyboard resizes.
    window_px: (i32, i32),
    /// Extra frames to ask for after startup or a resize while the Java side catches up.
    settle: u8,
    /// The opt-in frame-time benchmark, or `None` for normal reactive repainting.
    bench: Option<bench::FrameBench>,
}

impl eframe::App for Shell {
    /// The app-wide work: control channel, recovery timers, document sync, fonts and theme. Layout
    /// happens in [`Self::ui`], which eframe calls right after this.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
        self.watch_system_bars(ctx);
        if self.bench.as_ref().is_some_and(bench::FrameBench::wanted) {
            ctx.request_repaint();
        }
    }

    /// Hands egui the part of the surface the system bars leave free.
    ///
    /// `ANativeWindow` is the whole window surface, status bar and navigation/gesture bar
    /// included; `NativeActivity` lays its content view out inside them and stores the result in
    /// `AndroidApp::content_rect`. winit 0.30 drops the `ContentRectChanged` event, so the
    /// rectangle is read here every frame and turned into egui's safe area, which shrinks
    /// `Context::content_rect` (used by dialogs, tooltips and the layer clip) as well as the
    /// shell's own layout.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(window) = self.android_app.native_window() else {
            // No surface yet (before `Resumed`); leave the previous insets in place.
            return;
        };
        let raw = self.android_app.content_rect();
        let content = insets::ContentRect { left: raw.left, top: raw.top, right: raw.right, bottom: raw.bottom };
        let Some(pixels) = content.insets(window.width(), window.height()) else {
            // The Java content view has not been laid out yet; keep the last (safe) insets.
            return;
        };
        // The density begin_pass is about to use, without `RawInput::viewport`'s panic path.
        let native_ppp = raw_input.viewports.get(&raw_input.viewport_id).and_then(|viewport| viewport.native_pixels_per_point).unwrap_or(1.0);
        let Some([left, right, top, bottom]) = pixels.to_points(native_ppp * ctx.zoom_factor()) else {
            return;
        };
        // A rectangle that would leave no usable area is not a system bar; keep the last good
        // insets rather than clipping every layer of the interface away.
        if let Some(screen) = raw_input.screen_rect
            && !(screen.width() - left - right > 1.0 && screen.height() - top - bottom > 1.0)
        {
            return;
        }
        if self.content != Some(content) {
            self.content = Some(content);
            log::info!(
                "window content {}x{} at ({}, {}) px; system bars {} {} {} {} px",
                content.right.saturating_sub(content.left),
                content.bottom.saturating_sub(content.top),
                content.left,
                content.top,
                pixels.left,
                pixels.top,
                pixels.right,
                pixels.bottom,
            );
        }
        raw_input.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left, right, top, bottom }));
    }

    /// The real WordCraft interface: ribbon, rulers, page canvas, panes and status bar.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // eframe's root `Ui` still covers the whole viewport; lay the interface out only in the
        // part the system bars leave free. Everything WordCraft draws is a panel inside this `Ui`,
        // so the ribbon starts below the status bar and the status bar above the gesture bar.
        let safe = ui.ctx().content_rect();
        let area = ui.max_rect().intersect(safe);
        if area.width() > 1.0 && area.height() > 1.0 {
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| self.app.ui(ui));
        } else {
            // A stale or nonsensical rectangle must never blank the whole interface.
            self.app.ui(ui);
        }
        if let Some(bench) = &mut self.bench {
            bench.record(frame.info().cpu_usage);
        }
    }

    /// The strip under the status bar and around the edges is painted by eframe before egui runs;
    /// match the window chrome there instead of eframe's dark default.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    /// Finishes the recovery copy cleanly when the Activity ends. Android does not guarantee this
    /// hook, which is why recovery also runs continuously while the Activity is alive.
    fn on_exit(&mut self) {
        self.app.session.finish_recovery();
        log::info!("WordCraft: Activity finished");
        logging::flush();
    }
}

impl Shell {
    /// winit 0.30 ignores `MainEvent::ContentRectChanged`, so a changed content rectangle does not
    /// cause a frame of its own. Ask for another pass while the Java side has not laid the content
    /// view out yet (startup, Activity recreation), and one more after the surface changes size
    /// (rotation, split screen, keyboard) so the new insets are picked up.
    fn watch_system_bars(&mut self, ctx: &egui::Context) {
        let Some(window) = self.android_app.native_window() else {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        };
        let size = (window.width(), window.height());
        let raw = self.android_app.content_rect();
        let content = insets::ContentRect { left: raw.left, top: raw.top, right: raw.right, bottom: raw.bottom };
        if self.window_px != size || content.insets(size.0, size.1).is_none() {
            self.window_px = size;
            self.settle = 3;
        }
        if self.settle > 0 {
            self.settle -= 1;
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
