//! The single audited Android host module.
//!
//! `android_main` is the symbol `android-activity`'s GameActivity backend starts. Keep Android
//! JNI, `no_mangle` exports and platform-service bridges here so a reviewer can audit every
//! `unsafe` item in one file. The rest of the crate stays under `unsafe_code = "deny"`.

use eframe::egui;
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction, TextInputState};
use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

/// eframe application adapter. Android has no desktop-style title bar or native menu; the egui
/// front end draws the whole window.
struct AndroidShell {
    app: WordApp,
    ime: ImeBridge,
}

impl AndroidShell {
    fn new(app: WordApp, android: AndroidApp) -> Self {
        let ime = ImeBridge::new(android);
        Self { app, ime }
    }
}

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
        // Android's back gesture arrives as a viewport close request. `WordApp` asks before
        // discarding unsaved work; once it decides to quit, let eframe finish the activity.
        if self.app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
        self.ime.poll(&mut raw.events);
        // The soft keyboard has no text field of its own in the document: the document canvas
        // decides when Android should show it. Leaving the page for the ribbon drops a
        // composition that Gboard has not committed rather than typing it later.
        if !self.app.canvas.ime_wanted {
            self.ime.cancel_composition(&mut raw.events);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

/// GameActivity's text-input state is a small IME buffer, not the document. Composition stays in
/// the buffer and is shown as a preedit; when the IME ends composition (or commits plain text) the
/// new text goes to the document through egui's normal IME events. The buffer is emptied after
/// every commit so Gboard's next word starts from a clean slate.
struct ImeBridge {
    android: AndroidApp,
    /// The last state read from GameActivity, to notice changes: GameActivity exposes the current
    /// state but not its "changed" flag through `AndroidApp`.
    last_text: String,
    last_compose: Option<(usize, usize)>,
    preedit: String,
}

impl ImeBridge {
    fn new(android: AndroidApp) -> Self {
        // Text mode: sentences, autocorrect, multiple lines. Enter is a line break, not a form
        // action. Ask IMEs not to cover the whole screen with their extracted editor.
        android.set_ime_editor_info(
            InputType::TYPE_CLASS_TEXT
                | InputType::TYPE_TEXT_FLAG_CAP_SENTENCES
                | InputType::TYPE_TEXT_FLAG_AUTO_CORRECT
                | InputType::TYPE_TEXT_FLAG_MULTI_LINE
                | InputType::TYPE_TEXT_FLAG_IME_MULTI_LINE,
            TextInputAction::None,
            ImeOptions::IME_FLAG_NO_FULLSCREEN,
        );
        android.set_text_input_state(TextInputState::default());
        Self { android, last_text: String::new(), last_compose: None, preedit: String::new() }
    }

    fn reset_buffer(&self) {
        self.android.set_text_input_state(TextInputState::default());
    }

    /// Turns changes in GameActivity's IME buffer into egui's preedit/commit events.
    fn poll(&mut self, events: &mut Vec<egui::Event>) {
        let state = self.android.text_input_state();
        let compose = state.compose_region.map(|s| (s.start, s.end)).filter(|(start, end)| end > start);
        if state.text == self.last_text && compose == self.last_compose {
            return;
        }
        if compose.is_some() {
            // IMEs compose in place: the buffer holds the composition text. Show it under the
            // caret, with the document unchanged until the IME commits.
            if state.text != self.preedit {
                events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: state.text.clone(), active_range_chars: None }));
                self.preedit.clone_from(&state.text);
            }
            self.last_text.clone_from(&state.text);
            self.last_compose = compose;
            return;
        }
        let previous = std::mem::take(&mut self.last_text);
        let composing = !self.preedit.is_empty();
        if composing {
            // Composition ended: commit the result (or nothing, if the IME cancelled it).
            if state.text.is_empty() {
                events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
            } else {
                events.push(egui::Event::Ime(egui::ImeEvent::Commit(state.text.clone())));
            }
            self.preedit.clear();
        } else if let Some(inserted) = state.text.strip_prefix(previous.as_str()) {
            if !inserted.is_empty() {
                events.push(egui::Event::Ime(egui::ImeEvent::Commit(inserted.to_string())));
            }
        } else if let Some(remaining) = previous.strip_prefix(state.text.as_str()) {
            // The IME deleted from its buffer (Backspace in the suggestion strip).
            let removed = remaining.chars().count();
            if removed > 0 {
                events.push(egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: removed, after_chars: 0 }));
            }
        } else if !state.text.is_empty() {
            // A replacement that starts differently (autocorrect of a committed word): erase the
            // old buffer and type the new text. Both normally start from empty.
            let removed = previous.chars().count();
            if removed > 0 {
                events.push(egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { before_chars: removed, after_chars: 0 }));
            }
            events.push(egui::Event::Ime(egui::ImeEvent::Commit(state.text.clone())));
        }
        self.last_compose = None;
        self.reset_buffer();
        // The reset is ours, not an IME edit: don't read it back as a deletion next frame.
        self.last_text.clear();
    }

    /// Drops an unfinished composition when the page gives up the keyboard.
    fn cancel_composition(&mut self, events: &mut Vec<egui::Event>) {
        if self.preedit.is_empty() && self.last_text.is_empty() && self.last_compose.is_none() {
            return;
        }
        if !self.preedit.is_empty() {
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit { text: String::new(), active_range_chars: None }));
            self.preedit.clear();
        }
        // Keep the last text so a later poll sees no change; the IME buffer is emptied too.
        self.last_text.clear();
        self.last_compose = None;
        self.reset_buffer();
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

    let android_for_ui = app.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("WordCraft").with_inner_size([1200.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        android_app: Some(app),
        ..Default::default()
    };

    let result = eframe::run_native(
        "WordCraft",
        options,
        Box::new(move |_cc| {
            let mut session = Session::new(wordcraft_engine::sample::sample_document());
            session.system_region = wordcraft_ui_egui::i18n::system_region();

            let mut app = WordApp::new(session, Services::default());
            // M-A0 has no document URI yet. AutoSave stays off so the sample document never
            // tries to write a desktop path; SAF-backed AutoSave arrives with the storage arc.
            app.autosave = false;
            Ok(Box::new(AndroidShell::new(app, android_for_ui)))
        }),
    );

    if let Err(err) = result {
        log::error!("WordCraft Android stopped: {err}");
    }
}
