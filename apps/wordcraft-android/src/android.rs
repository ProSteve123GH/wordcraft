//! The single audited Android host module.
//!
//! `android_main` is the symbol `android-activity`'s GameActivity backend starts. Keep Android
//! JNI, `no_mangle` exports and platform-service bridges here so a reviewer can audit every
//! `unsafe` item in one file. The rest of the crate stays under `unsafe_code = "deny"`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;
use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::{Env, EnvUnowned, JValue};
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction, TextInputState};
use wordcraft_engine::Session;
use wordcraft_ui_egui::async_saves::{AsyncSaves, SaveOutcome};
use wordcraft_ui_egui::{Inbox, Services, UiState, WordApp};

/// eframe application adapter. Android has no desktop-style title bar or native menu; the egui
/// front end draws the whole window.
struct AndroidShell {
    app: WordApp,
    ime: ImeBridge,
    inbox: Inbox,
    saves: AsyncSaves,
    prefs: PrefsStore,
    /// The last `Canvas::soft_keyboard_request` seen. A page tap bumps it, and only then does the
    /// host ask Android to show the keyboard again (after Back, or after it was dismissed).
    last_soft_keyboard_request: u64,
}

impl AndroidShell {
    fn new(app: WordApp, android: AndroidApp, inbox: Inbox, saves: AsyncSaves, prefs: PrefsStore) -> Self {
        let ime = ImeBridge::new(android);
        Self { app, ime, inbox, saves, prefs, last_soft_keyboard_request: 0 }
    }

    /// Moves bytes and save outcomes from the Java picker threads into the app's own queues.
    fn pump_storage_results(&mut self) {
        while let Some(result) = next_open() {
            if !result.error.is_empty() {
                let name = clean_name(&result.name).unwrap_or_else(|| "the file".to_string());
                log::warn!("Android open failed for {name}: {}", result.error);
                self.app.status(format!("Couldn't open {name}: {}", result.error));
                continue;
            }
            if result.bytes.is_empty() {
                let name = clean_name(&result.name).unwrap_or_else(|| "the document".to_string());
                self.app.status(format!("Couldn't open {name}: the file is empty."));
                continue;
            }
            let inbox = self.inbox.clone();
            let mut files = lock(&inbox);
            if files.len() < MAX_INBOX_FILES {
                files.push((result.name, result.bytes));
            } else {
                log::warn!("dropping an opened file: too many are waiting");
            }
        }
        while let Some(result) = next_save() {
            let set_current = lock(pending_saves()).remove(&result.ticket).unwrap_or(false);
            let outcome = if result.ok {
                SaveOutcome::Saved
            } else if result.cancelled {
                SaveOutcome::Cancelled
            } else if result.error.is_empty() {
                SaveOutcome::Failed("the file could not be written".into())
            } else {
                SaveOutcome::Failed(result.error.clone())
            };
            if result.ok && set_current {
                self.note_saved_document(&result.name);
            }
            self.saves.finish(result.ticket, outcome);
        }
    }

    /// A document written through SAF is the document the app is editing: its display name
    /// becomes the title. `session.path` is only a name on Android; writes always go through the
    /// current SAF URI held by Java. Recent files need a stored URI to reopen, which this arc
    /// does not have yet, so they are deliberately not added here.
    fn note_saved_document(&mut self, name: &str) {
        let Some(name) = clean_name(name) else { return };
        self.app.session.path = Some(PathBuf::from(&name));
    }
}

impl Drop for AndroidShell {
    fn drop(&mut self) {
        // A system-initiated Activity destroy exits eframe without `quit_requested`; keep the
        // settings the last periodic write may have missed. Document recovery is separate.
        self.prefs.save_now(&self.app);
    }
}

impl eframe::App for AndroidShell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_storage_results();
        // Android Back: the keyboard goes first (it is the nearest thing), then the app's own
        // Back gesture. With nothing open it asks to close; a dirty document goes through Save
        // Changes, so unsaved work is never discarded here.
        if BACK_REQUESTED.swap(false, Ordering::SeqCst) {
            if IME_VISIBLE.swap(false, Ordering::SeqCst) {
                self.app.canvas.ime_wanted = false;
                self.ime.hide_soft_keyboard();
            } else {
                match self.app.back_gesture(ctx) {
                    wordcraft_ui_egui::BackGesture::Dismissed => {}
                    wordcraft_ui_egui::BackGesture::NothingOpen => {
                        // Clean or no document: the activity finishes now. A dirty document opens
                        // Save Changes instead (false), and the `quit_requested` pass below closes
                        // once it is answered.
                        if self.app.close_requested() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
        self.app.logic(ctx);
        // Settings live in the app's private storage; write them a few seconds after a change,
        // when the activity pauses, and once more when the user quits. A failure is only logged.
        if PAUSE_REQUESTED.swap(false, Ordering::SeqCst) {
            self.prefs.save_now(&self.app);
        }
        self.prefs.maybe_save(&self.app);
        // `close_requested` asks before discarding unsaved work; once it decides to quit, let
        // eframe finish the activity.
        if self.app.quit_requested {
            self.prefs.save_now(&self.app);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Java reports the system bars and display cutout in device-independent pixels. egui's
        // screen is in points after the interface zoom, so divide by it: the bars keep their
        // physical size at every Interface size, and the ribbon, page and status bar stay clear
        // of the cutout, the status bar and the navigation/gesture area.
        let zoom = ctx.zoom_factor();
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        let [left, top, right, bottom] = *lock(safe_insets());
        raw.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
            left: (left / zoom).clamp(0.0, 4096.0),
            right: (right / zoom).clamp(0.0, 4096.0),
            top: (top / zoom).clamp(0.0, 4096.0),
            bottom: (bottom / zoom).clamp(0.0, 4096.0),
        }));
        self.app.raw_input_hook(raw);
        self.ime.poll(&mut raw.events);
        // A real page tap asks for the keyboard even when Android hid it with Back: egui's
        // `o.ime` allowance does not change in that case, so `show_soft_input` is called
        // explicitly. Scrolling and ribbon taps never bump the counter.
        let request = self.app.canvas.soft_keyboard_request;
        if request != self.last_soft_keyboard_request {
            self.last_soft_keyboard_request = request;
            self.ime.show_soft_keyboard();
        }
        // The soft keyboard has no text field of its own in the document: the document canvas
        // decides when Android should show it. Leaving the page for the ribbon drops a
        // composition that Gboard has not committed rather than typing it later.
        if !self.app.canvas.ime_wanted {
            self.ime.cancel_composition(&mut raw.events);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
        // Show it in the same frame as the tap instead of waiting for the next input pass.
        let request = self.app.canvas.soft_keyboard_request;
        if request != self.last_soft_keyboard_request {
            self.last_soft_keyboard_request = request;
            self.ime.show_soft_keyboard();
        }
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
    fn show_soft_keyboard(&self) {
        // `false` asks for the keyboard now instead of only when Android considers it implicit.
        self.android.show_soft_input(false);
    }

    fn hide_soft_keyboard(&self) {
        // `false`: hide it even when an editor still nominally holds focus (Android Back).
        self.android.hide_soft_input(false);
    }

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

/// Largest number of picked files waiting for the app to open.
const MAX_INBOX_FILES: usize = 32;
/// Largest `ui.json` read back from private storage.
const MAX_PREFS_BYTES: u64 = 1 << 20;
/// How often changed settings are written while the app runs.
const PREFS_INTERVAL: Duration = Duration::from_secs(3);
const PREFS_FILE: &str = "ui.json";

/// Ribbon, theme, keymap and editing preferences between launches, kept in the app's private
/// `ui.json` (the same shape the desktop and web builds use). Writes are atomic and best-effort.
struct PrefsStore {
    path: Option<PathBuf>,
    last: Option<String>,
    last_write: Instant,
}

impl PrefsStore {
    fn new(dir: Option<PathBuf>) -> Self {
        let path = match dir {
            Some(dir) => Some(dir.join(PREFS_FILE)),
            None => {
                log::warn!("Android internal storage is unavailable; settings will not persist");
                None
            }
        };
        Self { path, last: None, last_write: Instant::now() }
    }

    /// Applies a stored `ui.json`; junk, missing and oversized files keep the defaults. Returns
    /// whether a settings file exists at all: `false` is a first run (the welcome tour shows).
    fn restore(&mut self, app: &mut WordApp) -> bool {
        let Some(path) = self.path.as_ref() else { return false };
        let size = match std::fs::metadata(path) {
            Ok(meta) => meta.len(),
            // No file: a first run. Later read or parse problems still count as an existing
            // profile (below), so a damaged `ui.json` never brings the tour back.
            Err(_) => return false,
        };
        if size > MAX_PREFS_BYTES {
            log::warn!("ignoring {}: it is {} bytes", path.display(), size);
            return true;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                return true;
            }
        };
        if let Some(ui) = UiState::from_json(&bytes) {
            app.apply_prefs(ui);
        }
        true
    }

    fn maybe_save(&mut self, app: &WordApp) {
        if self.path.is_none() {
            return;
        }
        let now = Instant::now();
        if now.saturating_duration_since(self.last_write) < PREFS_INTERVAL {
            return;
        }
        self.last_write = now;
        self.write_if_changed(app);
    }

    fn save_now(&mut self, app: &WordApp) {
        self.write_if_changed(app);
    }

    fn write_if_changed(&mut self, app: &WordApp) {
        let Some(path) = self.path.clone() else { return };
        let json = match serde_json::to_string(&app.prefs()) {
            Ok(json) => json,
            Err(e) => {
                log::warn!("cannot encode preferences: {e}");
                return;
            }
        };
        if self.last.as_deref() == Some(json.as_str()) {
            return;
        }
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, json.as_bytes()) {
            log::warn!("cannot write {}: {e}", tmp.display());
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            log::warn!("cannot replace {}: {e}", path.display());
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.last = Some(json);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn clean_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next()?.trim();
    if base.is_empty() {
        return None;
    }
    Some(base.chars().take(255).collect())
}

/// A file the Java picker read, waiting for the UI thread to hand it to the app.
struct OpenResult {
    name: String,
    #[allow(dead_code)]
    uri: String,
    bytes: Vec<u8>,
    error: String,
}

/// How a Java SAF write ended, waiting for the UI thread.
struct SaveResult {
    ticket: u64,
    ok: bool,
    cancelled: bool,
    name: String,
    #[allow(dead_code)]
    uri: String,
    error: String,
}

fn open_results() -> &'static Mutex<VecDeque<OpenResult>> {
    static RESULTS: OnceLock<Mutex<VecDeque<OpenResult>>> = OnceLock::new();
    RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn push_open(result: OpenResult) {
    let mut queue = lock(open_results());
    if queue.len() >= MAX_INBOX_FILES {
        queue.pop_front();
        log::warn!("dropping an older opened file: too many results arrived");
    }
    queue.push_back(result);
}

fn next_open() -> Option<OpenResult> {
    lock(open_results()).pop_front()
}

fn save_results() -> &'static Mutex<VecDeque<SaveResult>> {
    static RESULTS: OnceLock<Mutex<VecDeque<SaveResult>>> = OnceLock::new();
    RESULTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn push_save(result: SaveResult) {
    let mut queue = lock(save_results());
    if queue.len() >= MAX_INBOX_FILES {
        queue.pop_front();
        log::warn!("dropping an older save result: too many results arrived");
    }
    queue.push_back(result);
}

fn next_save() -> Option<SaveResult> {
    lock(save_results()).pop_front()
}

/// The save tickets whose write updates the document's current SAF URI.
fn pending_saves() -> &'static Mutex<HashMap<u64, bool>> {
    static PENDING: OnceLock<Mutex<HashMap<u64, bool>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Set by `pick_save` so the save that follows always opens the create-document picker.
static FORCE_SAVE_PICKER: AtomicBool = AtomicBool::new(false);

/// The egui context, so a picker thread can wake the UI when its result arrives.
fn ui_context() -> &'static Mutex<Option<egui::Context>> {
    static CONTEXT: OnceLock<Mutex<Option<egui::Context>>> = OnceLock::new();
    CONTEXT.get_or_init(|| Mutex::new(None))
}

fn set_ui_context(ctx: egui::Context) {
    *lock(ui_context()) = Some(ctx);
}

fn repaint_ui() {
    let ctx = lock(ui_context()).clone();
    if let Some(ctx) = ctx {
        ctx.request_repaint();
    }
}

/// The display's safe-area insets (system bars, cutout) in egui points, set by Java's window-insets
/// listener. `RawInput::safe_area_insets` keeps the last value when it is `None`, so this is fed
/// every frame; values are clamped so no display can make the content rect negative.
fn safe_insets() -> &'static Mutex<[f32; 4]> {
    static INSETS: Mutex<[f32; 4]> = Mutex::new([0.0; 4]);
    &INSETS
}

/// Java's Back was pressed (the new `OnBackInvokedCallback` or `onBackPressed`), to be handled on
/// the next UI pass where the document and dialogs live.
static BACK_REQUESTED: AtomicBool = AtomicBool::new(false);
/// The soft keyboard was visible in the last window insets. Back hides it first, as Android apps do.
static IME_VISIBLE: AtomicBool = AtomicBool::new(false);
/// The activity is pausing: flush settings at the next pass. A destroyed activity also saves on
/// drop, so a suspended loop loses nothing.
static PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The platform services: SAF pickers for opening, and writes through the current URI or a
/// create-document picker.
fn android_services(android: &AndroidApp, inbox: Inbox, saves: AsyncSaves) -> Services {
    let open_android = android.clone();
    let save_android = android.clone();
    let download_saves = saves.clone();
    Services {
        open_async: Some(Box::new(move |purpose: &str| launch_open(&open_android, purpose))),
        inbox: Some(inbox),
        pick_save: Some(Box::new(|name: &str| {
            FORCE_SAVE_PICKER.store(true, Ordering::SeqCst);
            Some(name.to_string())
        })),
        download: Some(Box::new(move |name: &str, bytes: &[u8]| {
            let ticket = download_saves.start();
            // A page image or PDF is a copy: it never becomes the document Save writes to.
            let set_current = !is_export_name(name);
            lock(pending_saves()).insert(ticket, set_current);
            let force_picker = FORCE_SAVE_PICKER.swap(false, Ordering::SeqCst);
            launch_save(&save_android, bytes, name, set_current, force_picker, ticket);
            Ok(())
        })),
        async_saves: Some(saves),
        // Fingers on a tablet: larger tap targets and spacing, and the taller ribbon strip.
        touch: true,
        ..Default::default()
    }
}

fn is_export_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".pdf") || lower.ends_with(".png")
}

fn launch_open(android: &AndroidApp, purpose: &str) {
    let callback = android.clone();
    let purpose = purpose.to_string();
    let multiple = purpose == "font";
    android.run_on_java_main_thread(Box::new(move || {
        with_activity(&callback, |env, activity| {
            let purpose = JString::new(env, &purpose)?;
            env.call_method(
                activity,
                jni::jni_str!("startOpenDocument"),
                jni::jni_sig!("(Ljava/lang/String;Z)V"),
                &[JValue::Object(purpose.as_ref()), JValue::Bool(multiple as jni::sys::jboolean)],
            )?;
            Ok(())
        });
    }));
}

fn launch_save(android: &AndroidApp, bytes: &[u8], name: &str, set_current: bool, force_picker: bool, ticket: u64) {
    let callback = android.clone();
    let bytes = bytes.to_vec();
    let name = name.to_string();
    android.run_on_java_main_thread(Box::new(move || {
        with_activity(&callback, |env, activity| {
            let data = env.byte_array_from_slice(&bytes)?;
            let name = JString::new(env, &name)?;
            env.call_method(
                activity,
                jni::jni_str!("startSaveDocument"),
                jni::jni_sig!("([BLjava/lang/String;ZZJ)V"),
                &[
                    JValue::Object(data.as_ref()),
                    JValue::Object(name.as_ref()),
                    JValue::Bool(set_current as jni::sys::jboolean),
                    JValue::Bool(force_picker as jni::sys::jboolean),
                    JValue::Long(ticket as jni::sys::jlong),
                ],
            )?;
            Ok(())
        });
    }));
}

/// Runs `call` on the Java main thread with the activity. Every failure is logged; none panic.
fn with_activity<F>(android: &AndroidApp, call: F)
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> jni::errors::Result<()>,
{
    let vm_ptr = android.vm_as_ptr();
    let activity_ptr = android.activity_as_ptr();
    if vm_ptr.is_null() || activity_ptr.is_null() {
        log::error!("Java VM or activity is unavailable");
        return;
    }
    // SAFETY: both pointers belong to the live AndroidApp and the Java VM it describes.
    let vm = unsafe { jni::vm::JavaVM::from_raw(vm_ptr.cast()) };
    let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        // SAFETY: the activity reference is valid for the activity's lifetime; the wrapper
        // borrows this JNI frame and is not stored.
        let activity = unsafe { JObject::from_raw(env, activity_ptr.cast()) };
        call(env, &activity)
    });
    if let Err(err) = result {
        log::error!("Android platform call failed: {err}");
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeOpenResult<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    data: JByteArray<'local>,
    name: JString<'local>,
    uri: JString<'local>,
    error: JString<'local>,
) {
    unowned_env
        .with_env(|env| -> jni::errors::Result<()> {
            let bytes = env.convert_byte_array(&data)?;
            let name = name.try_to_string(&*env)?;
            let uri = uri.try_to_string(&*env)?;
            let error = error.try_to_string(&*env)?;
            push_open(OpenResult { name, uri, bytes, error });
            repaint_ui();
            Ok(())
        })
        .resolve::<jni::errors::LogErrorAndDefault>();
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeSaveResult<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    ticket: jni::sys::jlong,
    ok: jni::sys::jboolean,
    cancelled: jni::sys::jboolean,
    name: JString<'local>,
    uri: JString<'local>,
    error: JString<'local>,
) {
    unowned_env
        .with_env(|env| -> jni::errors::Result<()> {
            let name = name.try_to_string(&*env)?;
            let uri = uri.try_to_string(&*env)?;
            let error = error.try_to_string(&*env)?;
            push_save(SaveResult { ticket: ticket.max(0) as u64, ok, cancelled, name, uri, error });
            repaint_ui();
            Ok(())
        })
        .resolve::<jni::errors::LogErrorAndDefault>();
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeInsets<'local>(
    _unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    left: jni::sys::jfloat,
    top: jni::sys::jfloat,
    right: jni::sys::jfloat,
    bottom: jni::sys::jfloat,
) {
    let mut values = [left, top, right, bottom];
    for v in &mut values {
        *v = if v.is_finite() { v.clamp(0.0, 4096.0) } else { 0.0 };
    }
    *lock(safe_insets()) = values;
    repaint_ui();
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeImeVisible<'local>(
    _unowned_env: EnvUnowned<'local>,
    _class: JClass<'local>,
    visible: jni::sys::jboolean,
) {
    IME_VISIBLE.store(visible != 0, Ordering::SeqCst);
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeBack<'local>(_unowned_env: EnvUnowned<'local>, _class: JClass<'local>) {
    BACK_REQUESTED.store(true, Ordering::SeqCst);
    repaint_ui();
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativePause<'local>(_unowned_env: EnvUnowned<'local>, _class: JClass<'local>) {
    PAUSE_REQUESTED.store(true, Ordering::SeqCst);
    repaint_ui();
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_storytold_wordcraft_MainActivity_nativeResume<'local>(_unowned_env: EnvUnowned<'local>, _class: JClass<'local>) {
    // Preferences stay live across a resume; asking for a frame picks up the bars and insets the
    // system may have moved while the activity was away.
    repaint_ui();
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

    let data_dir = app.internal_data_path();
    let android_for_services = app.clone();
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
        Box::new(move |cc| {
            // JNI callbacks from the picker threads wake the UI through this context.
            set_ui_context(cc.egui_ctx.clone());
            let inbox = Inbox::default();
            let saves = AsyncSaves::default();
            let services = android_services(&android_for_services, inbox.clone(), saves.clone());
            let mut prefs = PrefsStore::new(data_dir);

            // Android starts on File > Home, not on a built-in sample. The placeholder document
            // stays hidden until Blank document or Open creates or replaces it.
            let mut session = Session::new(wordcraft_engine::doc::Document::new());
            session.system_region = wordcraft_ui_egui::i18n::system_region();
            let mut app = WordApp::new(session, services);
            let returning = prefs.restore(&mut app);
            // First launch: the one-time welcome tour (the host stores its completion in ui.json).
            if !returning {
                app.ui.onboarding_done = false;
            }
            app.start_screen();
            // SAF save writes through `download`; AutoSave stays off until it can reuse the
            // current document URI safely on every path.
            app.autosave = false;
            Ok(Box::new(AndroidShell::new(app, android_for_ui, inbox, saves, prefs)))
        }),
    );

    if let Err(err) = result {
        log::error!("WordCraft Android stopped: {err}");
    }
}
