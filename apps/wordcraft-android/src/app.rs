//! The Android application shell: build the session, wrap `WordApp` in eframe's `App` trait and run
//! the event loop against the Activity's native window.
//!
//! Two layers sit between the platform and WordCraft's unchanged desktop UI:
//!
//! * [`crate::touchpad`] turns fingers into a virtual pointer, scroll, zoom and clicks, because a
//!   tablet has no mouse;
//! * [`crate::input`] carries the device list, edit state and Java window insets that winit's
//!   Android backend does not expose, from the Java `MainActivity` to this thread.
//!
//! The extra key bar is drawn in the shell (not in `ui-egui`), so the desktop and web builds do
//! not grow an Android-only row of keys.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use winit::platform::android::activity::AndroidApp;

use wordcraft_engine::Session;
use wordcraft_ui_egui::{Services, WordApp};

use crate::{bench, gpu, input, insets, logging, touchpad};

const APP_NAME: &str = "WordCraft";

/// Height of the Android-only key bar, including its margins.
const KEY_BAR_HEIGHT: f32 = 42.0;
/// A key button in the bar. The thirteen required keys fit on the tablet in portrait.
const KEY_SIZE: [f32; 2] = [38.0, 30.0];

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

    // Java's Back handler uses this to show/hide the keyboard through android-activity's own
    // path, which the IME is attached to.
    input::set_android_app(android_app.clone());
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
            let screen = cc.egui_ctx.content_rect();
            let bounds = touchpad::Bounds::new(touchpad::Point::new(screen.min.x, screen.min.y), touchpad::Point::new(screen.max.x, screen.max.y));
            log::info!("gestures: touch is a touchpad; two fingers scroll/pinch, three tap or swipe down for the key bar");
            Ok(Box::new(Shell {
                app,
                android_app: insets_app,
                content: None,
                java_insets: None,
                window_px: (0, 0),
                settle: 3,
                bench: bench_config.map(bench::FrameBench::new),
                touchpad: touchpad::Touchpad::new(bounds),
                keys_visible: true,
                sticky: egui::Modifiers::NONE,
                pending_keys: Vec::new(),
                restore_focus: None,
                last_focus: None,
                last_modifiers: egui::Modifiers::NONE,
                physical: egui::Modifiers::NONE,
                last_devices: input::Devices::default(),
                last_touch_time: 0.0,
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
    /// The `WindowInsets` last reported by Java, so a change is logged once and the content-rect
    /// fallback is only used before Java has reported anything.
    java_insets: Option<input::WindowInsets>,
    /// The surface size in physical pixels, to notice rotation and keyboard resizes.
    window_px: (i32, i32),
    /// Extra frames to ask for after startup or a resize while the Java side catches up.
    settle: u8,
    /// The opt-in frame-time benchmark, or `None` for normal reactive repainting.
    bench: Option<bench::FrameBench>,
    /// Fingers to the virtual pointer, scroll, zoom and clicks.
    touchpad: touchpad::Touchpad,
    /// Whether the Android-only extra key bar is shown (three-finger swipe down toggles it).
    keys_visible: bool,
    /// Ctrl/Alt/Shift tapped in the key bar and applied to the next injected key.
    sticky: egui::Modifiers,
    /// Keys tapped in the key bar, injected at the start of the next frame.
    pending_keys: Vec<egui::Key>,
    /// The editor focus to restore after a key-bar click; a click on a button makes a `TextEdit`
    /// surrender focus, and the injected key must still reach it.
    restore_focus: Option<egui::Id>,
    /// The last focused widget seen before the key bar took a click.
    last_focus: Option<egui::Id>,
    /// The modifier keys winit last reported as held. Android's backend does not send
    /// `ModifiersChanged`, so the shell tracks the left/right modifier keys itself.
    physical: egui::Modifiers,
    /// The modifier state last sent to egui, to emit `ModifiersChanged` only on change.
    last_modifiers: egui::Modifiers,
    /// For logging when a physical mouse, touchpad or keyboard is attached or removed.
    last_devices: input::Devices,
    /// Time used when the platform does not provide `RawInput::time`.
    last_touch_time: f64,
}

impl eframe::App for Shell {
    /// The app-wide work: control channel, recovery timers, document sync, fonts and theme. Layout
    /// happens in [`Self::ui`], which eframe calls right after this.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
        // Android does not guarantee a release for every key: a modifier held while the app loses
        // focus can stick down forever. Reset the tracked state when the window is unfocused; the
        // next `raw_input_hook` tells egui with `ModifiersChanged`.
        if !ctx.input(|input| input.focused) {
            self.physical = egui::Modifiers::NONE;
        }
        // The Java Back handler asks this: while a text editor wants the keyboard, Back toggles
        // the IME instead of leaving the Activity. The document page is a focusable canvas rather
        // than an egui `TextEdit`, so check its flag as well. Deliberately not
        // `egui_wants_keyboard_input`: that is true for any focused button too, and Back must keep
        // its normal meaning (close the dialog) when a button has focus.
        let editing = self.app.canvas.focused || ctx.text_edit_focused();
        input::set_editing(editing);
        // Remember the editor before a key-bar click clears it; `raw_input_hook` restores it on
        // the next frame (see `queue_key`). Only a focused editor is a useful target: updating
        // from the key bar's own buttons would restore focus to a button instead of the page or
        // text box after a sticky modifier was tapped.
        let focused = ctx.memory(|memory| memory.focused());
        if editing && let Some(id) = focused {
            self.last_focus = Some(id);
        } else if !editing && focused.is_none() {
            // Clicking the canvas or another non-focusable area drops the editor focus; do not
            // resurrect an old editor when a key is tapped later.
            self.last_focus = None;
        }
        let devices = input::devices();
        if devices != self.last_devices {
            log::info!("input devices: mouse={} touchpad={} keyboard={}", devices.mouse, devices.touchpad, devices.keyboard);
            self.last_devices = devices;
        }
        self.watch_system_bars(ctx);
        if self.bench.as_ref().is_some_and(bench::FrameBench::wanted) {
            ctx.request_repaint();
        }
    }

    /// Hands egui the part of the surface the system bars leave free, tracks the physical keyboard
    /// modifiers winit does not report, and turns touch events into touchpad actions.
    ///
    /// `ANativeWindow` is the whole window surface, status bar and navigation/gesture bar
    /// included; `NativeActivity` lays its content view out inside them and stores the result in
    /// `AndroidApp::content_rect`. winit 0.30 drops the `ContentRectChanged` event, so the
    /// rectangle is read here every frame and turned into egui's safe area, which shrinks
    /// `Context::content_rect` (used by dialogs, tooltips and the layer clip) as well as the
    /// shell's own layout.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Undo the focus loss from a key-bar click before this frame's events are processed.
        if let Some(id) = self.restore_focus.take() {
            ctx.memory_mut(|memory| memory.request_focus(id));
            // The page reads its `focused` flag in this pass before it is refreshed from egui's
            // focus state, so a restored key would be dropped without this.
            if self.app.canvas.widget() == Some(id) {
                self.app.canvas.focused = true;
            }
        }

        let bounds = Self::content_bounds(ctx);
        self.touchpad.set_bounds(bounds);
        let density = raw_input.viewports.get(&raw_input.viewport_id).and_then(|viewport| viewport.native_pixels_per_point).unwrap_or(1.0);
        let density = if density.is_finite() && density > 0.0 { density * ctx.zoom_factor() } else { 1.0 };
        // winit's Android backend delivers the left/right modifier keys as ordinary key events but
        // never sends `ModifiersChanged` and puts no modifier state on the following key. Track
        // them here and decorate every key event, or Ctrl/Alt/Shift shortcuts never fire. The key
        // bar's sticky modifiers are added on top, so Ctrl+arrow and Shift+arrow work from the bar
        // too. The state change is announced before the event that caused it.
        let mut used_modifiers = false;
        let pending = std::mem::take(&mut raw_input.events);
        let mut decorated = Vec::with_capacity(pending.len() + 4);
        let initial = self.event_modifiers();
        if initial != self.last_modifiers {
            decorated.push(egui::Event::ModifiersChanged(initial));
            self.last_modifiers = initial;
        }
        for mut event in pending {
            if let egui::Event::Key { key, pressed, modifiers, .. } = &mut event {
                self.track_modifier(*key, *pressed);
                let mods = self.event_modifiers();
                if mods != self.last_modifiers {
                    decorated.push(egui::Event::ModifiersChanged(mods));
                    self.last_modifiers = mods;
                }
                *modifiers = mods;
                // A modifier key press only changes the state above; it must not consume a sticky
                // modifier tapped in the bar, so Ctrl+Shift+arrow with a sticky Ctrl still works.
                used_modifiers |= *pressed && !Self::is_modifier_key(*key);
            }
            decorated.push(event);
        }
        raw_input.events = decorated;

        // Touch is not passed to egui: the recogniser consumes it and emits pointer/scroll/zoom
        // events in its place. Doing it here means the app's `logic` and `ui` see a normal frame.
        let now = raw_input.time.unwrap_or_else(|| self.last_touch_time + f64::from(raw_input.predicted_dt.max(0.0)));
        self.last_touch_time = now;
        let pending = std::mem::take(&mut raw_input.events);
        let mut kept = Vec::with_capacity(pending.len());
        let mut actions = Vec::new();
        let mut touch_started = false;
        for event in pending {
            match event {
                egui::Event::Touch { id, phase, pos, .. } => {
                    let phase = match phase {
                        egui::TouchPhase::Start => touchpad::TouchPhase::Start,
                        egui::TouchPhase::Move => touchpad::TouchPhase::Move,
                        egui::TouchPhase::End => touchpad::TouchPhase::End,
                        egui::TouchPhase::Cancel => touchpad::TouchPhase::Cancel,
                    };
                    touch_started |= phase == touchpad::TouchPhase::Start;
                    actions.extend(self.touchpad.process(touchpad::TouchSample::new(id.0, phase, touchpad::Point::new(pos.x, pos.y), now)));
                }
                other => kept.push(other),
            }
        }
        raw_input.events = kept;
        self.touchpad.tick(now);
        // A finger held still produces no events, so egui might not run another frame and the
        // long-press drag would never arm. One frame just after the threshold is enough.
        if touch_started {
            ctx.request_repaint_after(Duration::from_millis(520));
        }
        self.apply_actions(&actions, &mut raw_input.events);

        // Keys tapped in the bar, with the same modifiers as keyboard events, so Ctrl+arrow and
        // Shift+arrow work. Sticky modifiers are cleared after the key, like a real keyboard.
        if !self.pending_keys.is_empty() {
            used_modifiers = true;
            let inject_modifiers = self.event_modifiers();
            for key in self.pending_keys.drain(..) {
                raw_input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: inject_modifiers });
                raw_input.events.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: inject_modifiers });
            }
        }
        if used_modifiers {
            self.sticky = egui::Modifiers::NONE;
        }

        self.apply_safe_area(raw_input, density);
    }

    /// Java reports the real `WindowInsets` from `onApplyWindowInsets`. Prefer them: on devices
    /// that lay the native surface out edge to edge (Android 15's forced edge-to-edge for target
    /// 35+, and several OEM themes) the content rectangle is the whole window and would put the
    /// ribbon under the status bar. Until Java reports real insets, fall back to `content_rect`.
    fn apply_safe_area(&mut self, raw_input: &mut egui::RawInput, density: f32) {
        if let Some(java) = input::window_insets() {
            let left = java.left as f32 / density;
            let right = java.right as f32 / density;
            let top = java.top as f32 / density;
            let bottom = java.bottom as f32 / density;
            let usable = raw_input.screen_rect.is_none_or(|screen| screen.width() - left - right > 1.0 && screen.height() - top - bottom > 1.0);
            // An all-zero report before the first layout is not authoritative; let the content
            // rectangle answer until Java reports real bars.
            if usable && (java.left > 0 || java.top > 0 || java.right > 0 || java.bottom > 0 || self.java_insets.is_some()) {
                if self.java_insets != Some(java) {
                    self.java_insets = Some(java);
                    log::info!(
                        "window insets {} {} {} {} px; safe area {} {} {} {} pt",
                        java.left,
                        java.top,
                        java.right,
                        java.bottom,
                        left,
                        top,
                        right,
                        bottom
                    );
                }
                raw_input.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left, right, top, bottom }));
                return;
            }
        }

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
        let Some([left, right, top, bottom]) = pixels.to_points(density) else {
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
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                if self.keys_visible {
                    self.key_bar(ui);
                }
                self.app.ui(ui);
                self.paint_pointer(ui);
            });
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
    fn content_bounds(ctx: &egui::Context) -> touchpad::Bounds {
        let rect = ctx.content_rect();
        touchpad::Bounds::new(touchpad::Point::new(rect.min.x, rect.min.y), touchpad::Point::new(rect.max.x, rect.max.y))
    }

    fn gesture_button(button: touchpad::Button) -> egui::PointerButton {
        match button {
            touchpad::Button::Left => egui::PointerButton::Primary,
            touchpad::Button::Right => egui::PointerButton::Secondary,
            touchpad::Button::Middle => egui::PointerButton::Middle,
        }
    }

    /// Whether a key is one of the modifier keys tracked by [`Self::track_modifier`].
    fn is_modifier_key(key: egui::Key) -> bool {
        matches!(
            key,
            egui::Key::ShiftLeft
                | egui::Key::ShiftRight
                | egui::Key::ControlLeft
                | egui::Key::ControlRight
                | egui::Key::AltLeft
                | egui::Key::AltRight
                | egui::Key::SuperLeft
                | egui::Key::SuperRight
        )
    }

    /// Update the physical modifier state from a key event. winit's Android backend reports the
    /// left/right modifier keys as distinct key codes but sends no `ModifiersChanged`; the key
    /// events themselves are the only source of this state.
    fn track_modifier(&mut self, key: egui::Key, pressed: bool) {
        match key {
            egui::Key::ShiftLeft | egui::Key::ShiftRight => self.physical.shift = pressed,
            egui::Key::ControlLeft | egui::Key::ControlRight => self.physical.ctrl = pressed,
            egui::Key::AltLeft | egui::Key::AltRight => self.physical.alt = pressed,
            egui::Key::SuperLeft | egui::Key::SuperRight => self.physical.mac_cmd = pressed,
            _ => {}
        }
    }

    /// The modifiers for an event: physical keys plus the key bar's sticky modifiers.
    fn event_modifiers(&self) -> egui::Modifiers {
        self.modifiers_with(true)
    }

    /// Only the physical modifiers, for pointer events: a sticky Ctrl tapped in the key bar must
    /// not turn a tap into a Ctrl-click or a two-finger scroll into a zoom.
    fn physical_modifiers(&self) -> egui::Modifiers {
        self.modifiers_with(false)
    }

    fn modifiers_with(&self, sticky: bool) -> egui::Modifiers {
        let ctrl = self.physical.ctrl || (sticky && self.sticky.ctrl);
        let mac_cmd = self.physical.mac_cmd;
        egui::Modifiers {
            alt: self.physical.alt || (sticky && self.sticky.alt),
            ctrl,
            shift: self.physical.shift || (sticky && self.sticky.shift),
            mac_cmd,
            command: ctrl || mac_cmd,
        }
    }

    fn queue_key(&mut self, ctx: &egui::Context, key: egui::Key) {
        self.pending_keys.push(key);
        self.restore_editor_focus(ctx);
        ctx.request_repaint();
    }

    /// Put keyboard focus back on the editor after a key-bar button stole it. Clicking a button
    /// makes the editor surrender focus, and with it the IME: egui would ask the platform to hide
    /// the keyboard for this frame. Restoring before the canvas draws (it comes after the bar)
    /// keeps the keyboard up and lets the injected key land in the editor next frame.
    fn restore_editor_focus(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.last_focus {
            ctx.memory_mut(|memory| memory.request_focus(id));
            // The page reads its `focused` flag in this pass before refreshing it from egui's focus
            // state, so a restored key would be dropped without this.
            if self.app.canvas.widget() == Some(id) {
                self.app.canvas.focused = true;
            }
            self.restore_focus = Some(id);
        }
    }

    fn apply_actions(&mut self, actions: &[touchpad::Action], events: &mut Vec<egui::Event>) {
        let modifiers = self.physical_modifiers();
        for action in actions {
            match *action {
                touchpad::Action::MovePointer { pos } => {
                    events.push(egui::Event::PointerMoved(Self::pos(pos)));
                }
                touchpad::Action::Press { button, pos } => {
                    let pos = Self::pos(pos);
                    events.push(egui::Event::PointerMoved(pos));
                    events.push(egui::Event::PointerButton { pos, button: Self::gesture_button(button), pressed: true, modifiers });
                }
                touchpad::Action::Release { button, pos } => {
                    let pos = Self::pos(pos);
                    events.push(egui::Event::PointerButton { pos, button: Self::gesture_button(button), pressed: false, modifiers });
                }
                touchpad::Action::Click { button, pos } => {
                    let pos = Self::pos(pos);
                    let button = Self::gesture_button(button);
                    events.push(egui::Event::PointerMoved(pos));
                    // Down and up in one frame still counts as a click in egui: the release sees
                    // the press that was processed just before it.
                    events.push(egui::Event::PointerButton { pos, button, pressed: true, modifiers });
                    events.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers });
                }
                touchpad::Action::Scroll { delta, phase } => {
                    events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(delta.x, delta.y),
                        phase: if phase == touchpad::ScrollPhase::End { egui::TouchPhase::End } else { egui::TouchPhase::Move },
                        source: egui::MouseWheelSource::Trackpad,
                        modifiers,
                    });
                }
                touchpad::Action::Zoom { factor } => {
                    if factor.is_finite() && factor > 0.0 {
                        events.push(egui::Event::Zoom(factor));
                    }
                }
                touchpad::Action::ToggleKeyBar => {
                    self.keys_visible = !self.keys_visible;
                }
            }
        }
    }

    fn pos(point: touchpad::Point) -> egui::Pos2 {
        egui::pos2(point.x, point.y)
    }

    /// The Android-only extra key bar. It is a normal egui panel so the ribbon, canvas and status
    /// bar above it shrink rather than being covered, and it stays above the IME because the
    /// Activity's window is resized by the keyboard.
    fn key_bar(&mut self, ui: &mut egui::Ui) {
        let fill = ui.visuals().panel_fill;
        egui::Panel::bottom("android_key_bar")
            .exact_size(KEY_BAR_HEIGHT)
            .show_separator_line(true)
            .frame(egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(6, 4)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 0.0);
                    if ui.add_sized(KEY_SIZE, egui::Button::selectable(self.sticky.ctrl, "Ctrl")).clicked() {
                        self.sticky.ctrl = !self.sticky.ctrl;
                        self.restore_editor_focus(ui.ctx());
                    }
                    if ui.add_sized(KEY_SIZE, egui::Button::selectable(self.sticky.alt, "Alt")).clicked() {
                        self.sticky.alt = !self.sticky.alt;
                        self.restore_editor_focus(ui.ctx());
                    }
                    if ui.add_sized(KEY_SIZE, egui::Button::selectable(self.sticky.shift, "Shift")).clicked() {
                        self.sticky.shift = !self.sticky.shift;
                        self.restore_editor_focus(ui.ctx());
                    }
                    self.key_button(ui, "Esc", egui::Key::Escape);
                    self.key_button(ui, "Tab", egui::Key::Tab);
                    self.key_button(ui, "←", egui::Key::ArrowLeft);
                    self.key_button(ui, "→", egui::Key::ArrowRight);
                    self.key_button(ui, "↑", egui::Key::ArrowUp);
                    self.key_button(ui, "↓", egui::Key::ArrowDown);
                    self.key_button(ui, "Home", egui::Key::Home);
                    self.key_button(ui, "End", egui::Key::End);
                    self.key_button(ui, "PgUp", egui::Key::PageUp);
                    self.key_button(ui, "PgDn", egui::Key::PageDown);
                });
            });
    }

    fn key_button(&mut self, ui: &mut egui::Ui, label: &str, key: egui::Key) {
        if ui.add_sized(KEY_SIZE, egui::Button::new(label)).clicked() {
            self.queue_key(ui.ctx(), key);
        }
    }

    /// Draws the touchpad cursor above every panel. It is the only pointer the shell controls;
    /// winit's Android backend discards physical mouse hover, wheel and extra-button events.
    fn paint_pointer(&self, ui: &egui::Ui) {
        let point = self.touchpad.pointer();
        let pos = Self::pos(point);
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("android_virtual_pointer"));
        let painter = ui.ctx().layer_painter(layer);
        let fill = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220);
        let ink = egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230);
        painter.circle_filled(pos, 7.0, fill);
        painter.circle_stroke(pos, 7.0, egui::Stroke::new(1.5, ink));
        painter.circle_filled(pos, 1.5, ink);
    }

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
        if self.window_px != size || content.insets(size.0, size.1).is_none() || input::take_insets_dirty() {
            self.window_px = size;
            self.settle = 3;
        }
        if self.settle > 0 {
            self.settle -= 1;
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
