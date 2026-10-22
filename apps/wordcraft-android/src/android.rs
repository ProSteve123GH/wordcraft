//! The single audited Android host module.
//!
//! `android_main` is the symbol `android-activity`'s GameActivity backend starts. Keep Android
//! JNI, `no_mangle` exports and platform-service bridges here so a reviewer can audit every
//! `unsafe` item in one file. The rest of the crate stays under `unsafe_code = "deny"`.

use eframe::egui;
use winit::platform::android::activity::AndroidApp;
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

/// eframe application adapter. Android has no desktop-style title bar or native menu; the egui
/// front end draws the whole window.
struct AndroidShell(WordApp);

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
        // Android's back gesture arrives as a viewport close request. `WordApp` asks before
        // discarding unsaved work; once it decides to quit, let eframe finish the activity.
        if self.0.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.0.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

/// Native entry point called by GameActivity through `android-activity`.
///
/// The export is the only `no_mangle` symbol in the Android crate; the enclosing module is the
/// only place the workspace's `unsafe_code = "forbid"` is relaxed.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("WordCraft"));
    log::info!("WordCraft Android starting");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("WordCraft").with_inner_size([1200.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        android_app: Some(app),
        ..Default::default()
    };

    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(|_cc| {
            let mut session = Session::new(wordcraft_engine::sample::sample_document());
            session.system_region = wordcraft_ui_egui::i18n::system_region();

            let mut app = WordApp::new(session, Services::default());
            // M-A0 has no document URI yet. AutoSave stays off so the sample document never
            // tries to write a desktop path; SAF-backed AutoSave arrives with the storage arc.
            app.autosave = false;
            Ok(Box::new(AndroidShell(app)))
        }),
    );

    if let Err(err) = result {
        log::error!("WordCraft Android stopped: {err}");
    }
}
