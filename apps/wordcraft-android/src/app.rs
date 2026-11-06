//! The Android application shell: build the session, wrap `WordApp` in eframe's `App` trait and run
//! the event loop against the Activity's native window.

use std::sync::Arc;

use eframe::egui;
use winit::platform::android::activity::AndroidApp;

use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

use crate::logging;

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

    let options = eframe::NativeOptions {
        android_app: Some(android_app),
        viewport: egui::ViewportBuilder::default().with_title(APP_NAME).with_app_id("ai.storyteller.wordcraft"),
        ..Default::default()
    };
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
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
            Ok(Box::new(Shell { app }))
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
}

impl eframe::App for Shell {
    /// The app-wide work: control channel, recovery timers, document sync, fonts and theme. Layout
    /// happens in [`Self::ui`], which eframe calls right after this.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
    }

    /// The real WordCraft interface: ribbon, rulers, page canvas, panes and status bar.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }

    /// Finishes the recovery copy cleanly when the Activity ends. Android does not guarantee this
    /// hook, which is why recovery also runs continuously while the Activity is alive.
    fn on_exit(&mut self) {
        self.app.session.finish_recovery();
        log::info!("WordCraft: Activity finished");
        logging::flush();
    }
}
