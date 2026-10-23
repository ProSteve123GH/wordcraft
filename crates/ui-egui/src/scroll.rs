//! How the document canvas scrolls (issue #122).
//!
//! egui's built-in wheel handling smooths every delta it thinks is coarse and drops the deltas of
//! a touchpad's first and last events, and Wayland compositors leave kinetic scrolling to the app
//! (GTK, Qt and browsers synthesise it after the fingers lift). So the canvas scrolls itself:
//! - touchpad (pixel) deltas move the page 1:1, with no animation;
//! - after a touchpad flick the page coasts and slows down ([`Momentum`]), on platforms whose
//!   system doesn't already send momentum events (not macOS, Windows or the web);
//! - a mouse-wheel notch scrolls about three text lines and eases in over ~0.1 s, like egui;
//! - on a touch screen two fingers pan the page 1:1 and coast when they lift, and a pinch zooms
//!   around the point between the fingers ([`CanvasScroll::zoom`]).
//!
//! With animation effects off ([`crate::theme::Animations`]) nothing eases: a notch scrolls the
//! page its whole distance at once, and so do egui's own scroll areas ([`unsmoothed_scroll`]).
//! Ctrl/⌘+wheel zooms in whole steps, each at once ([`CanvasScroll::zoom`]), and a pinch applies
//! each frame's factor as it comes. Touchpad momentum stays: it carries on the fingers' own
//! movement, as the momentum macOS and Windows send does whatever their animation setting.

use egui::{Event, InputOptions, Modifiers, MouseWheelUnit, TouchPhase, Vec2};

/// Screen points one mouse-wheel notch scrolls at 100% zoom: about three lines of body text,
/// like other desktop apps (egui's default is 40).
pub const NOTCH_PX: f32 = 56.0;

/// Whether to synthesise kinetic scrolling. macOS and Windows send their own momentum events
/// (or report touchpads as line deltas), and browsers do their own; only Linux/BSD need it.
const SYNTH_MOMENTUM: bool = !cfg!(any(target_os = "macos", target_os = "ios", target_os = "windows", target_arch = "wasm32"));

/// Longest frame time trusted for velocity and decay (a stall shouldn't fling the page).
const MAX_DT: f32 = 0.1;
/// Shortest event spacing trusted for velocity (two events in one burst aren't infinitely fast).
const MIN_DT: f32 = 1.0 / 500.0;
/// Velocity is averaged over roughly this many seconds of input.
const VELOCITY_WINDOW: f32 = 0.04;
/// With no "fingers lifted" event, coast after this long without input.
const IDLE_RELEASE: f32 = 0.05;
/// Fingers that rested this long before lifting don't fling.
const STALE: f32 = 0.1;
/// Slower flicks than this (points/s) don't coast…
const MIN_FLING: f32 = 150.0;
/// …and coasting stops below this speed.
const STOP_SPEED: f32 = 20.0;
/// Fastest coast (points/s).
const MAX_SPEED: f32 = 8000.0;
/// Exponential decay rate of the coast (1/s): speed falls to 1/e every ~0.33 s.
const DECAY: f32 = 3.0;

/// With animation effects off, Ctrl/⌘+wheel zooms in steps of this size (the natural logarithm of
/// the factor: about 10%). A mouse-wheel notch is two steps at once, the same zoom as with
/// animation effects on. Turns that arrive in small pieces (a touchpad pinch that Windows reports
/// as Ctrl+wheel, a free-spinning or high-resolution wheel) add up to a step first, so the zoom
/// moves in a few plain steps instead of gliding.
const ZOOM_STEP: f32 = 0.1;
/// Part of a zoom step is forgotten after this many seconds without more Ctrl/⌘+wheel input.
const ZOOM_STEP_IDLE: f64 = 0.75;
/// Most zoom steps taken from one frame's input (the zoom itself is clamped to 10–500%).
const MAX_ZOOM_STEPS: f32 = 64.0;

/// Kinetic scrolling for touchpads: tracks the scroll velocity while the fingers move and keeps
/// scrolling with exponentially decaying speed after they lift. Time-based, so frame-rate
/// independent. Deltas are in screen points, positive = content moves down/right.
#[derive(Clone, Debug, Default)]
pub struct Momentum {
    velocity: Vec2,
    /// Seconds since the last fed delta.
    since_input: f32,
    /// Fingers are on the touchpad (deltas arrived and no lift yet).
    streaming: bool,
    coasting: bool,
    /// The device reports when the fingers lift, so don't guess it from a pause.
    reports_lift: bool,
}

impl Momentum {
    /// A frame's touchpad delta, `dt` seconds after the previous frame. Non-finite input is ignored.
    pub fn feed(&mut self, delta: Vec2, dt: f32) {
        if !delta.is_finite() || !dt.is_finite() {
            return;
        }
        let dt = dt.clamp(0.0, MAX_DT);
        let fresh = !self.streaming || self.since_input + dt > STALE;
        let span = if fresh { dt } else { self.since_input + dt }.clamp(MIN_DT, MAX_DT);
        let sample = delta / span;
        self.velocity = if fresh {
            sample
        } else {
            let a = 1.0 - (-span / VELOCITY_WINDOW).exp();
            self.velocity + (sample - self.velocity) * a
        };
        let speed = self.velocity.length();
        if speed > MAX_SPEED {
            self.velocity *= MAX_SPEED / speed;
        }
        self.since_input = 0.0;
        self.streaming = true;
        self.coasting = false;
    }

    /// The fingers lifted: coast if the flick was fast and recent enough.
    pub fn release(&mut self) {
        self.reports_lift = true;
        self.start_coast();
    }

    fn start_coast(&mut self) {
        let fling = self.streaming && self.since_input <= STALE && self.velocity.length() >= MIN_FLING;
        self.streaming = false;
        if fling {
            self.coasting = true;
        } else {
            self.cancel();
        }
    }

    /// Stop dead (a click, a key, a wheel notch, the end of the document).
    pub fn cancel(&mut self) {
        self.velocity = Vec2::ZERO;
        self.coasting = false;
        self.streaming = false;
    }

    /// Advance a frame without touchpad input; returns how far to coast this frame.
    pub fn tick(&mut self, dt: f32) -> Vec2 {
        if !dt.is_finite() || dt <= 0.0 {
            return Vec2::ZERO;
        }
        let dt = dt.min(MAX_DT);
        self.since_input += dt;
        if self.streaming && !self.reports_lift && self.since_input >= IDLE_RELEASE {
            self.start_coast();
        }
        if !self.coasting {
            return Vec2::ZERO;
        }
        // Exact integral of v·e^(−kt) over the frame, so the distance doesn't depend on frame rate.
        let keep = (-DECAY * dt).exp();
        let step = self.velocity * ((1.0 - keep) / DECAY);
        self.velocity *= keep;
        if self.velocity.length() < STOP_SPEED {
            self.cancel();
        }
        step
    }

    /// Still moving or waiting to decide whether to coast: keep repainting.
    pub fn is_active(&self) -> bool {
        self.coasting || (self.streaming && !self.reports_lift)
    }
}

/// The canvas's scrolling state: momentum plus the not-yet-scrolled part of wheel notches, and
/// the part of a zoom step the wheel has turned so far.
#[derive(Clone, Debug, Default)]
pub struct CanvasScroll {
    momentum: Momentum,
    /// Wheel distance still to scroll, eased out over a few frames.
    pending: Vec2,
    /// Ctrl/⌘+wheel zoom not yet applied (natural logarithm, less than a [`ZOOM_STEP`]), and the
    /// input time it last grew. Only used with animation effects off.
    zoom_pending: f32,
    zoom_time: f64,
    /// Two fingers were on the screen last frame, so the frame they lift is the pan's release.
    touch_panning: bool,
    /// A pinch's intended scale and the part shown so far, both since the pinch began. Easing the
    /// shown scale towards the intended one keeps a shaky pinch from jittering the page; `None`
    /// when no pinch is in progress or catching up. Only used with animation effects on.
    pinch: Option<(f32, f32)>,
}

impl CanvasScroll {
    /// This frame's scroll delta in screen points (positive = content moves down/right) from the
    /// wheel/touchpad events in `input`. `hovered`: the pointer is over the canvas (only then do
    /// its wheel events scroll it). `notch`: points per wheel notch. `page`: viewport height.
    /// `animate`: animation effects are on, so wheel notches ease in rather than land at once.
    pub fn frame(&mut self, input: &egui::InputState, opts: &InputOptions, hovered: bool, notch: f32, page: f32, animate: bool) -> Vec2 {
        let notch = if notch.is_finite() { notch.clamp(1.0, 1000.0) } else { NOTCH_PX };
        let page = if page.is_finite() { page.clamp(0.0, 100_000.0) } else { 0.0 };
        // Browsers report lines (Firefox: three per notch) where native platforms report notches.
        let line = if cfg!(target_arch = "wasm32") { notch / 3.0 } else { notch };
        let dt = input.unstable_dt;
        let mut now = Vec2::ZERO;
        let mut touchpad = Vec2::ZERO;
        let mut lifted = false;
        let mut stop = false;
        let mut zooming = false;
        for event in &input.events {
            match event {
                Event::MouseWheel { unit, delta, phase, modifiers } => {
                    // Ctrl/⌘+wheel zooms (egui turns it into a zoom factor).
                    if !hovered {
                        continue;
                    }
                    if modifiers.matches_any(opts.zoom_modifier) {
                        zooming = true;
                        continue;
                    }
                    let d = axis_lock(if delta.is_finite() { *delta } else { Vec2::ZERO }, *modifiers, opts);
                    match unit {
                        // Touchpads (and precise mice): 1:1. Browsers also report wheel notches in
                        // points, so big jumps there still ease in, like egui does.
                        MouseWheelUnit::Point if !cfg!(target_arch = "wasm32") || d.length() < 8.0 => touchpad += d,
                        MouseWheelUnit::Point => self.pending += d,
                        // Whole notches ease in; fractional ones (X11/Windows touchpads, free-spinning
                        // wheels) are already smooth.
                        MouseWheelUnit::Line if d.x.fract() == 0.0 && d.y.fract() == 0.0 => {
                            stop = true;
                            self.pending += d * line;
                        }
                        MouseWheelUnit::Line => now += d * line,
                        MouseWheelUnit::Page => self.pending += d * page,
                    }
                    lifted |= matches!(phase, TouchPhase::End | TouchPhase::Cancel);
                }
                Event::Key { pressed: true, .. } | Event::PointerButton { pressed: true, .. } | Event::Text(_) => stop = true,
                _ => {}
            }
        }
        // The wheel zooms this frame: whatever was still gliding (a coast, a notch easing in)
        // stops rather than scroll the page on top of the zoom's anchor correction (#1743). The early
        // return deliberately drops this frame's other deltas too.
        if zooming {
            self.hit_edge();
            return Vec2::ZERO;
        }
        // Two fingers on the glass pan the page 1:1 (one finger belongs to the text: selection,
        // dragging an object). The average finger movement scrolls the same way a touchpad delta
        // does, and the same coast follows when they lift, so a flick carries on smoothly.
        let mut touch_panning = false;
        if let Some(touch) = input.multi_touch() {
            touch_panning = true;
            let d = touch.translation_delta;
            if d.is_finite() {
                touchpad += d;
            }
        }
        now += touchpad;
        let fed = touchpad != Vec2::ZERO;
        if SYNTH_MOMENTUM {
            if fed {
                self.momentum.feed(touchpad, dt);
            }
            if lifted || (self.touch_panning && !touch_panning) {
                self.momentum.release();
            }
        }
        self.touch_panning = touch_panning;
        // A click, key or wheel notch stops a coast (even one that started this frame).
        if stop {
            self.momentum.cancel();
        }
        if SYNTH_MOMENTUM && !fed {
            now += self.momentum.tick(dt);
        }
        // Ease wheel notches in: 90% of the way in 0.1 s (egui's curve). Without animation effects
        // the whole distance lands this frame.
        if self.pending != Vec2::ZERO {
            let t = if animate && dt.is_finite() { egui::emath::exponential_smooth_factor(0.9, 0.1, dt.clamp(0.0, MAX_DT)) } else { 1.0 };
            let ease = |pending: f32| if pending.abs() < 1.0 { pending } else { pending * t };
            let step = Vec2::new(ease(self.pending.x), ease(self.pending.y));
            now += step;
            self.pending -= step;
        }
        now
    }

    /// The scroll ran into the end of the document: stop coasting and drop queued notches.
    pub fn hit_edge(&mut self) {
        self.momentum.cancel();
        self.pending = Vec2::ZERO;
    }

    /// A zoom took over the wheel: drop any coast or queued notches so they don't scroll the page
    /// on top of the anchor correction that keeps the pointer's page point in place (#1743).
    pub fn cancel(&mut self) {
        self.hit_edge();
    }

    /// Scrolling or a pinch easing continues without new input: keep repainting.
    pub fn is_animating(&self) -> bool {
        self.pending != Vec2::ZERO || self.momentum.is_active() || self.pinch.is_some()
    }

    /// This frame's zoom factor from Ctrl/⌘+wheel and pinch gestures (1.0: none). With `animate`
    /// it is egui's, which eases a wheel notch's zoom in over a few frames. Without, the wheel
    /// zooms in whole steps ([`ZOOM_STEP`]), each at once, however finely the turns arrive; a
    /// pinch the system reports as a pinch still follows the fingers.
    pub fn zoom(&mut self, input: &egui::InputState, opts: &InputOptions, animate: bool) -> f32 {
        // Two fingers on a touch screen: the pinch follows the fingers, but with animation effects
        // on each frame eases into the next scale instead of jumping with every shiver. The
        // intended and shown scales are kept for the whole pinch, so the last part still arrives
        // after the fingers lift (and the page keeps its anchor point while it does).
        if let Some(touch) = input.multi_touch() {
            self.zoom_pending = 0.0;
            let raw = if touch.zoom_delta.is_finite() && touch.zoom_delta > 0.0 { touch.zoom_delta } else { 1.0 };
            if !animate {
                self.pinch = None;
                return raw;
            }
            let (mut target, shown) = self.pinch.unwrap_or((1.0, 1.0));
            target = (target * raw).clamp(0.2, 5.0);
            let next = ease_step(shown, target, input.stable_dt);
            self.pinch = Some((target, next));
            return pinch_factor(next, shown);
        }
        // Fingers have lifted, but the last part of the pinch may still be arriving.
        if let Some((target, shown)) = self.pinch {
            self.zoom_pending = 0.0;
            if !animate {
                self.pinch = None;
                return 1.0;
            }
            let next = ease_step(shown, target, input.stable_dt);
            self.pinch = if (target - next).abs() <= 1e-4 { None } else { Some((target, next)) };
            return pinch_factor(next, shown);
        }
        // Ctrl/⌘+wheel and a pinch the system reports as a zoom factor: egui's own easing, or the
        // whole-step zoom below.
        if animate {
            self.zoom_pending = 0.0;
            return input.zoom_delta();
        }
        let mut zoom = 1.0_f32;
        let mut turned = 0.0_f32;
        for event in &input.events {
            match event {
                Event::Zoom(factor) => zoom *= factor,
                Event::MouseWheel { unit, delta, phase: TouchPhase::Move, modifiers } if modifiers.matches_any(opts.zoom_modifier) => {
                    let d = egui_points(*unit, *delta, input, opts);
                    turned += opts.scroll_zoom_speed * (d.x + d.y);
                }
                _ => {}
            }
        }
        if turned != 0.0 {
            if input.time - self.zoom_time > ZOOM_STEP_IDLE {
                self.zoom_pending = 0.0;
            }
            self.zoom_time = input.time;
            let total = self.zoom_pending + turned;
            // A hair of tolerance: a notch's two steps mustn't come out as 1.9999.
            let steps = if total.is_finite() { (total / ZOOM_STEP + 1e-3_f32.copysign(total)).trunc() } else { 0.0 };
            self.zoom_pending = if total.is_finite() { total - steps * ZOOM_STEP } else { 0.0 };
            zoom *= (steps.clamp(-MAX_ZOOM_STEPS, MAX_ZOOM_STEPS) * ZOOM_STEP).exp();
        }
        if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 }
    }
}

/// One frame of easing from `shown` towards `target`: 90% of the gap closes in 60 ms. Hostile
/// frame times leave the scale where it is (and the next frame catches up).
fn ease_step(shown: f32, target: f32, dt: f32) -> f32 {
    if !shown.is_finite() || shown <= 0.0 {
        return 1.0;
    }
    if !target.is_finite() || target <= 0.0 {
        return shown;
    }
    if !dt.is_finite() || dt <= 0.0 {
        return shown;
    }
    let a = egui::emath::exponential_smooth_factor(0.9, 0.06, dt.clamp(0.0, MAX_DT));
    shown + (target - shown) * a
}

/// The zoom factor to apply for a pinch whose shown scale went from `shown` to `next`; junk never
/// comes out as a factor.
fn pinch_factor(next: f32, shown: f32) -> f32 {
    if !next.is_finite() || !shown.is_finite() || shown <= 0.0 {
        return 1.0;
    }
    let factor = next / shown;
    if factor.is_finite() && factor > 0.0 { factor } else { 1.0 }
}

/// Shift scrolls sideways and Alt scrolls vertically, like egui's own scroll areas.
fn axis_lock(d: Vec2, modifiers: Modifiers, opts: &InputOptions) -> Vec2 {
    let horizontal = modifiers.matches_any(opts.horizontal_scroll_modifier);
    let vertical = modifiers.matches_any(opts.vertical_scroll_modifier);
    match (horizontal, vertical) {
        (true, false) => Vec2::new(d.x + d.y, 0.0),
        (false, true) => Vec2::new(0.0, d.x + d.y),
        _ => d,
    }
}

/// A wheel event's delta in points the way egui's own scroll areas count it: a line is
/// `line_scroll_speed` points and a page is the window's height.
fn egui_points(unit: MouseWheelUnit, delta: Vec2, input: &egui::InputState, opts: &InputOptions) -> Vec2 {
    let d = match unit {
        MouseWheelUnit::Point => delta,
        MouseWheelUnit::Line => delta * opts.line_scroll_speed,
        MouseWheelUnit::Page => delta * input.viewport_rect().height(),
    };
    if d.is_finite() { d } else { Vec2::ZERO }
}

/// This frame's wheel scrolling for egui's own scroll areas (panes, lists, dialogs, Backstage)
/// with nothing held back: egui eases a wheel notch in over the next frames
/// (`InputState::smooth_scroll_delta`) and has no switch for it, so with animation effects off the
/// app puts this in its place every frame. Ctrl/⌘+wheel is left out, as in egui: it zooms.
pub fn unsmoothed_scroll(input: &egui::InputState, opts: &InputOptions) -> Vec2 {
    let mut total = Vec2::ZERO;
    for event in &input.events {
        if let Event::MouseWheel { unit, delta, phase: TouchPhase::Move, modifiers } = event
            && !modifiers.matches_any(opts.zoom_modifier)
        {
            total += axis_lock(egui_points(*unit, *delta, input, opts), *modifiers, opts);
        }
    }
    if total.is_finite() { total } else { Vec2::ZERO }
}

/// Points per wheel notch at `zoom` (1.0 = 100%): more when zoomed in, less when zoomed out, but
/// gently, so zoomed-out overviews don't crawl and close-ups don't jump.
pub fn notch_px(zoom: f32) -> f32 {
    let z = if zoom.is_finite() && zoom > 0.0 { zoom.sqrt().clamp(0.75, 2.0) } else { 1.0 };
    NOTCH_PX * z
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn flick(m: &mut Momentum) {
        for _ in 0..6 {
            m.feed(Vec2::new(0.0, -30.0), FRAME);
        }
    }

    #[test]
    fn flick_coasts_then_decays_to_a_stop() {
        let mut m = Momentum::default();
        flick(&mut m);
        m.release();
        let first = m.tick(FRAME);
        assert!(first.y < -10.0, "keeps going the same way after the fingers lift: {first:?}");
        let mut total = first.y;
        let mut last = first.y.abs();
        let mut frames = 0;
        while m.is_active() {
            let d = m.tick(FRAME).y;
            assert!(d.abs() <= last + 1e-3, "slows down");
            last = d.abs();
            total += d;
            frames += 1;
            assert!(frames < 600, "stops within ten seconds");
        }
        // 1800 pt/s with a 1/3 s time constant: a light coast of about 600 pt.
        assert!((-700.0..-400.0).contains(&total), "coasted {total}");
        assert_eq!(m.tick(FRAME), Vec2::ZERO);

        // Frame-rate independent: 144 Hz coasts about as far.
        let mut fast = Momentum::default();
        flick(&mut fast);
        fast.release();
        let mut total144 = 0.0;
        while fast.is_active() {
            total144 += fast.tick(1.0 / 144.0).y;
        }
        assert!((total144 - total).abs() < 30.0, "{total144} vs {total}");
    }

    #[test]
    fn new_input_cancels_and_slow_or_stale_lifts_dont_fling() {
        let mut m = Momentum::default();
        flick(&mut m);
        m.release();
        assert_ne!(m.tick(FRAME), Vec2::ZERO);
        m.cancel();
        assert!(!m.is_active());
        assert_eq!(m.tick(FRAME), Vec2::ZERO);

        // Fingers touching again mid-coast take over: no coasting until they lift again.
        flick(&mut m);
        m.release();
        m.feed(Vec2::new(0.0, -1.0), FRAME);
        assert_eq!(m.tick(FRAME), Vec2::ZERO, "the coast stopped");
        m.release();
        assert!(!m.is_active(), "a slow drag doesn't fling");

        // Resting the fingers before lifting doesn't fling either.
        flick(&mut m);
        for _ in 0..10 {
            m.tick(FRAME);
        }
        m.release();
        assert!(!m.is_active());
    }

    fn wheel(unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers) -> Event {
        Event::MouseWheel { unit, delta, phase: TouchPhase::Move, modifiers }
    }

    /// One 60 fps frame over a tall egui scroll area with the pointer on it; `animate` false puts
    /// [`unsmoothed_scroll`] in place of egui's eased delta, as the app does. Returns the scroll
    /// area's offset and the frame's [`CanvasScroll::zoom`] (its state is kept with the context).
    fn egui_frame(ctx: &egui::Context, frame_no: &mut u32, animate: bool, mut events: Vec<Event>) -> (f32, f32) {
        *frame_no += 1;
        events.insert(0, Event::PointerMoved(egui::pos2(50.0, 50.0)));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let input = egui::RawInput { events, time: Some(f64::from(*frame_no) / 60.0), screen_rect: Some(screen), ..Default::default() };
        let mut seen = (0.0, 1.0);
        ctx.run_ui(input, |ui| {
            let opts = ui.ctx().options(|o| o.input_options);
            if !animate {
                ui.ctx().input_mut(|i| i.smooth_scroll_delta = unsmoothed_scroll(i, &opts));
            }
            let id = egui::Id::new("canvas scroll under test");
            let mut scroll = ui.data(|d| d.get_temp::<CanvasScroll>(id)).unwrap_or_default();
            let zoom = ui.input(|i| scroll.zoom(i, &opts, animate));
            ui.data_mut(|d| d.insert_temp(id, scroll));
            let out = egui::ScrollArea::vertical().show(ui, |ui| ui.allocate_space(egui::vec2(100.0, 5000.0)));
            seen = (out.state.offset.y, zoom);
        })
        .drop_without_applying_deltas();
        seen
    }

    /// One frame of [`CanvasScroll::frame`] with the pointer on a 400x300 screen.
    fn canvas_frame(ctx: &egui::Context, scroll: &mut CanvasScroll, t: f64, events: Vec<Event>) -> Vec2 {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let input = egui::RawInput { events, time: Some(t), screen_rect: Some(screen), ..Default::default() };
        let mut out = Vec2::ZERO;
        ctx.run_ui(input, |ui| {
            let opts = ui.ctx().options(|o| o.input_options);
            out = ui.input(|i| scroll.frame(i, &opts, true, NOTCH_PX, 300.0, true));
        })
        .drop_without_applying_deltas();
        out
    }

    /// Issue #1743: a Ctrl/⌘+wheel event drops queued wheel notches instead of scrolling them on top
    /// of the zoom. (The touchpad-coast case is covered by `ctrl_wheel_zoom_does_not_scroll_the_document`.)
    #[test]
    fn zoom_modifier_wheel_clears_queued_notches() {
        for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
            let ctx = egui::Context::default();
            let mut scroll = CanvasScroll::default();
            let mut t = 0.0;
            let mut next = || {
                t += f64::from(FRAME);
                t
            };
            canvas_frame(&ctx, &mut scroll, next(), vec![Event::PointerMoved(egui::pos2(50.0, 50.0))]);
            // A fresh fling and a queued notch.
            for _ in 0..6 {
                canvas_frame(&ctx, &mut scroll, next(), vec![wheel(MouseWheelUnit::Point, Vec2::new(0.0, -30.0), Modifiers::NONE)]);
            }
            canvas_frame(&ctx, &mut scroll, next(), vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, -5.0), Modifiers::NONE)]);
            assert!(scroll.is_animating(), "something is still gliding");
            let moved = canvas_frame(&ctx, &mut scroll, next(), vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, 1.0), modifiers)]);
            assert_eq!(moved, Vec2::ZERO, "the zoom frame scrolls nothing ({modifiers:?})");
            assert!(!scroll.is_animating(), "nothing glides on after the zoom ({modifiers:?})");
            for _ in 0..5 {
                assert_eq!(canvas_frame(&ctx, &mut scroll, next(), Vec::new()), Vec2::ZERO, "no leftover scroll ({modifiers:?})");
            }
        }
    }

    /// egui's own scroll areas ease a wheel notch in over several frames; with animation effects
    /// off the notch lands whole in its frame, and nothing more arrives afterwards.
    #[test]
    fn egui_scroll_areas_take_a_wheel_notch_at_once_without_animation() {
        let line = InputOptions::default().line_scroll_speed;
        let notch = || vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, -1.0), Modifiers::NONE)];
        let settle = |ctx: &egui::Context, n: &mut u32, animate| (0..40).map(|_| egui_frame(ctx, n, animate, Vec::new()).0).last().unwrap();

        let (ctx, mut n) = (egui::Context::default(), 0);
        egui_frame(&ctx, &mut n, true, Vec::new());
        let first = egui_frame(&ctx, &mut n, true, notch()).0;
        assert!(first > 0.0 && first < line, "egui eases the notch in: {first}");
        assert!((settle(&ctx, &mut n, true) - line).abs() < 0.5);

        let (ctx, mut n) = (egui::Context::default(), 0);
        egui_frame(&ctx, &mut n, false, Vec::new());
        assert_eq!(egui_frame(&ctx, &mut n, false, notch()).0, line, "the whole notch in one frame");
        assert_eq!(settle(&ctx, &mut n, false), line, "and none of egui's leftover easing after it");
        // Shift turns the notch sideways, and Ctrl/⌘+wheel zooms rather than scrolls, as in egui.
        assert_eq!(egui_frame(&ctx, &mut n, false, vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, -1.0), Modifiers::SHIFT)]).0, line);
        assert_eq!(egui_frame(&ctx, &mut n, false, vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, -1.0), Modifiers::COMMAND)]).0, line);
        // Touchpad deltas pass through as they are.
        assert_eq!(egui_frame(&ctx, &mut n, false, vec![wheel(MouseWheelUnit::Point, Vec2::new(0.0, -3.0), Modifiers::NONE)]).0, line + 3.0);
        // A junk delta scrolls nothing. Last: egui's own wheel state doesn't recover from it.
        let junk = vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, f32::NEG_INFINITY), Modifiers::NONE)];
        assert_eq!(egui_frame(&ctx, &mut n, false, junk).0, line + 3.0);
    }

    /// Ctrl/⌘+wheel: egui spreads a notch's zoom over several frames. With animation effects off
    /// the wheel zooms in whole steps, each at once: a notch is two steps in its own frame, and
    /// turns that arrive in small pieces add up to a step first rather than each zooming a little.
    #[test]
    fn wheel_zoom_steps_at_once_without_animation() {
        let opts = InputOptions::default();
        let notch_zoom = (opts.scroll_zoom_speed * opts.line_scroll_speed).exp();
        let step = ZOOM_STEP.exp();
        let turn = |notches: f32| vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, notches), Modifiers::COMMAND)];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;

        let (ctx, mut n) = (egui::Context::default(), 0);
        egui_frame(&ctx, &mut n, true, Vec::new());
        let first = egui_frame(&ctx, &mut n, true, turn(1.0)).1;
        assert!(first > 1.0 && first < notch_zoom, "egui eases the zoom in: {first} of {notch_zoom}");

        let (ctx, mut n) = (egui::Context::default(), 0);
        egui_frame(&ctx, &mut n, false, Vec::new());
        let zoom = egui_frame(&ctx, &mut n, false, turn(1.0)).1;
        assert!(near(zoom, notch_zoom) && near(zoom, step * step), "a notch is two steps at once: {zoom}");
        assert_eq!(egui_frame(&ctx, &mut n, false, Vec::new()).1, 1.0, "nothing left for later frames");
        assert!(near(egui_frame(&ctx, &mut n, false, turn(-1.0)).1, 1.0 / notch_zoom));
        // Quarter notches, as a touchpad pinch or a free-spinning wheel sends them: a step with
        // every second one and nothing in between, never a little with each.
        let zooms: Vec<f32> = (0..4).map(|_| egui_frame(&ctx, &mut n, false, turn(0.25)).1).collect();
        assert!(zooms[0] == 1.0 && near(zooms[1], step) && zooms[2] == 1.0 && near(zooms[3], step), "{zooms:?}");
        let zooms: Vec<f32> = (0..4).map(|_| egui_frame(&ctx, &mut n, false, turn(-0.25)).1).collect();
        assert!(zooms[0] == 1.0 && near(zooms[1], 1.0 / step) && zooms[2] == 1.0 && near(zooms[3], 1.0 / step), "{zooms:?}");
        // Part of a step doesn't wait for ever: after a pause the count starts again.
        assert_eq!(egui_frame(&ctx, &mut n, false, turn(0.25)).1, 1.0);
        for _ in 0..60 {
            assert_eq!(egui_frame(&ctx, &mut n, false, Vec::new()).1, 1.0);
        }
        assert_eq!(egui_frame(&ctx, &mut n, false, turn(0.25)).1, 1.0, "the quarter turn of a second ago has lapsed");
        assert!(near(egui_frame(&ctx, &mut n, false, turn(0.25)).1, step));
        // A pinch the system reports as a pinch arrives as a zoom factor of its own and follows the
        // fingers; a plain notch doesn't zoom.
        assert_eq!(egui_frame(&ctx, &mut n, false, vec![Event::Zoom(1.5)]).1, 1.5);
        assert_eq!(egui_frame(&ctx, &mut n, false, vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, 1.0), Modifiers::NONE)]).1, 1.0);
        // An absurd turn is a bounded zoom, and junk never comes out as a zoom factor (the endless
        // wheel delta last, as above).
        let huge = egui_frame(&ctx, &mut n, false, turn(1e6)).1;
        assert!(huge > notch_zoom && huge <= (MAX_ZOOM_STEPS * ZOOM_STEP).exp() * 1.001, "{huge}");
        for junk in [Event::Zoom(f32::NAN), Event::Zoom(0.0), wheel(MouseWheelUnit::Page, Vec2::new(0.0, f32::INFINITY), Modifiers::COMMAND)] {
            assert_eq!(egui_frame(&ctx, &mut n, false, vec![junk]).1, 1.0);
        }
    }

    const TOUCH_DEVICE: egui::TouchDeviceId = egui::TouchDeviceId(0);

    /// One finger going down, moving or lifting at `(x, y)`.
    fn finger(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
        Event::Touch { device_id: TOUCH_DEVICE, id: egui::TouchId(id), phase, pos: egui::pos2(x, y), force: None }
    }

    /// One frame with `events`, returning both the frame's scroll and its zoom factor. The pointer
    /// sits at the gesture's centre, as a touch screen reports it (egui only starts a gesture when
    /// it knows where the pointer is).
    fn touch_frame(ctx: &egui::Context, scroll: &mut CanvasScroll, t: f64, mut events: Vec<Event>, pointer: egui::Pos2) -> (Vec2, f32) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        events.push(Event::PointerMoved(pointer));
        let input = egui::RawInput { events, time: Some(t), screen_rect: Some(screen), ..Default::default() };
        let mut seen = (Vec2::ZERO, 1.0);
        ctx.run_ui(input, |ui| {
            let opts = ui.ctx().options(|o| o.input_options);
            seen = ui.input(|i| (scroll.frame(i, &opts, true, NOTCH_PX, 300.0, true), scroll.zoom(i, &opts, true)));
        })
        .drop_without_applying_deltas();
        seen
    }

    /// Two fingers on the glass pan the page 1:1, one finger leaves it to the text, and the pan
    /// coasts after the fingers lift like a touchpad flick.
    #[test]
    fn two_finger_pan_moves_the_page_and_coasts() {
        let ctx = egui::Context::default();
        let mut scroll = CanvasScroll::default();
        let mut t = 0.0;
        let mut next = || {
            t += f64::from(FRAME);
            t
        };
        // One finger: no pan.
        touch_frame(&ctx, &mut scroll, next(), vec![finger(1, TouchPhase::Start, 100.0, 100.0)], egui::pos2(100.0, 100.0));
        let (one, _) = touch_frame(&ctx, &mut scroll, next(), vec![finger(1, TouchPhase::Move, 100.0, 40.0)], egui::pos2(100.0, 40.0));
        assert_eq!(one, Vec2::ZERO, "one finger selects text; it doesn't scroll");
        touch_frame(&ctx, &mut scroll, next(), vec![finger(1, TouchPhase::End, 100.0, 40.0)], egui::pos2(100.0, 40.0));

        // Two fingers, both moving up 6 pt a frame: the page follows 1:1.
        touch_frame(
            &ctx,
            &mut scroll,
            next(),
            vec![finger(1, TouchPhase::Start, 100.0, 100.0), finger(2, TouchPhase::Start, 140.0, 100.0)],
            egui::pos2(120.0, 100.0),
        );
        let mut panned = Vec2::ZERO;
        for i in 0..6 {
            let y = 100.0 - 6.0 * (i as f32 + 1.0);
            let (d, _) = touch_frame(
                &ctx,
                &mut scroll,
                next(),
                vec![finger(1, TouchPhase::Move, 100.0, y), finger(2, TouchPhase::Move, 140.0, y)],
                egui::pos2(120.0, y),
            );
            panned += d;
        }
        assert!(panned.y < -30.0 && panned.y > -40.0, "the page followed the fingers: {panned:?}");

        // Both lift: where the platform doesn't send its own momentum, the pan coasts on in the
        // same direction, then settles (macOS, Windows and the web send momentum themselves).
        touch_frame(
            &ctx,
            &mut scroll,
            next(),
            vec![finger(1, TouchPhase::End, 100.0, 64.0), finger(2, TouchPhase::End, 140.0, 64.0)],
            egui::pos2(120.0, 64.0),
        );
        if SYNTH_MOMENTUM {
            assert!(scroll.is_animating(), "the pan coasts after the lift");
            let mut coast = Vec2::ZERO;
            for _ in 0..5 {
                let (d, _) = touch_frame(&ctx, &mut scroll, next(), Vec::new(), egui::pos2(120.0, 64.0));
                coast += d;
            }
            assert!(coast.y < -5.0, "the coast carries the fingers' movement: {coast:?}");
        }
    }

    /// One frame's zoom alone (`scroll.frame` and `scroll.zoom`, as the canvas runs them).
    fn zoom_frame(ctx: &egui::Context, scroll: &mut CanvasScroll, t: f64, animate: bool, mut events: Vec<Event>, pointer: egui::Pos2) -> f32 {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        events.push(Event::PointerMoved(pointer));
        let input = egui::RawInput { events, time: Some(t), screen_rect: Some(screen), ..Default::default() };
        let zoom = std::cell::Cell::new(1.0_f32);
        ctx.run_ui(input, |ui| {
            let opts = ui.ctx().options(|o| o.input_options);
            let z = ui.input(|i| {
                let _ = scroll.frame(i, &opts, true, NOTCH_PX, 300.0, animate);
                scroll.zoom(i, &opts, animate)
            });
            zoom.set(z);
        })
        .drop_without_applying_deltas();
        zoom.get()
    }

    /// A pinch zooms around the point between the fingers. With animation effects on the shown
    /// scale eases towards the fingers (so a shaky pinch doesn't jitter the page) and the rest
    /// arrives after they lift; with them off each frame's factor lands at once.
    #[test]
    fn pinch_eases_to_the_fingers_then_finishes() {
        let centre = egui::pos2(100.0, 150.0);
        let start = || vec![finger(1, TouchPhase::Start, 90.0, 150.0), finger(2, TouchPhase::Start, 110.0, 150.0)];
        // Fingers 20 pt apart, then 40: a ×2 pinch.
        let apart = || vec![finger(1, TouchPhase::Move, 80.0, 150.0), finger(2, TouchPhase::Move, 120.0, 150.0)];
        let lift = || vec![finger(1, TouchPhase::End, 80.0, 150.0), finger(2, TouchPhase::End, 120.0, 150.0)];

        let ctx = egui::Context::default();
        let mut scroll = CanvasScroll::default();
        let mut frame = 0.0_f64;
        let at = |f: &mut f64| {
            *f += f64::from(FRAME);
            *f
        };
        assert_eq!(zoom_frame(&ctx, &mut scroll, at(&mut frame), true, start(), centre), 1.0);
        let first = zoom_frame(&ctx, &mut scroll, at(&mut frame), true, apart(), centre);
        assert!(first > 1.0 && first < 2.0, "eased, not jumped: {first}");
        let mut total = first;
        // Fingers rest at the same spread; the shown scale catches up to them.
        while scroll.is_animating() && frame < 200.0 {
            total *= zoom_frame(&ctx, &mut scroll, at(&mut frame), true, apart(), centre);
        }
        // Lift: any last part still arrives.
        total *= zoom_frame(&ctx, &mut scroll, at(&mut frame), true, lift(), centre);
        while scroll.is_animating() && frame < 400.0 {
            total *= zoom_frame(&ctx, &mut scroll, at(&mut frame), true, Vec::new(), centre);
        }
        assert!(!scroll.is_animating(), "the pinch settles");
        assert!((total - 2.0).abs() < 0.05, "the whole pinch arrived: ×{total}");

        // Without animation: the first frame is the fingers' own factor and nothing follows.
        let mut scroll = CanvasScroll::default();
        let mut frame = 0.0_f64;
        assert_eq!(zoom_frame(&ctx, &mut scroll, at(&mut frame), false, start(), centre), 1.0);
        let zoom = zoom_frame(&ctx, &mut scroll, at(&mut frame), false, apart(), centre);
        assert!((zoom - 2.0).abs() < 1e-4, "each frame's factor at once: ×{zoom}");
        assert!(!scroll.is_animating(), "nothing left to ease");
    }

    #[test]
    fn hostile_numbers_are_ignored() {
        let mut m = Momentum::default();
        m.feed(Vec2::new(f32::NAN, f32::INFINITY), FRAME);
        m.feed(Vec2::new(0.0, -30.0), f32::NAN);
        assert!(!m.is_active());
        flick(&mut m);
        m.feed(Vec2::new(0.0, f32::NEG_INFINITY), FRAME);
        m.release();
        assert_eq!(m.tick(f32::NAN), Vec2::ZERO);
        assert_eq!(m.tick(f32::INFINITY), Vec2::ZERO);
        let d = m.tick(FRAME);
        assert!(d.is_finite() && d.y < 0.0);
        // A huge delta in a tiny frame is capped.
        let mut m = Momentum::default();
        m.feed(Vec2::new(0.0, 1e30), 1e-9);
        m.release();
        assert!(m.tick(FRAME).length() <= MAX_SPEED * FRAME);
        assert!(notch_px(f32::NAN).is_finite() && notch_px(-1.0) == NOTCH_PX);
    }
}
