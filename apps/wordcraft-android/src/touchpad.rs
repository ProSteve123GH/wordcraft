//! Touch gestures for the Android shell.
//!
//! The tablet is used without a mouse, but WordCraft is a desktop word processor: a finger has to
//! become a pointer, a swipe has to move it, and a tap has to click where it is. This is the
//! touchpad model Termux:X11 uses on Android, and it is why plain touch input is not passed to
//! egui directly:
//!
//! * one finger moves the virtual pointer; a tap clicks, a second tap and drag selects text;
//! * two fingers scroll (never select), pinch zooms, or a quick tap right-clicks;
//! * three fingers tap to middle-click, or swipe down to show/hide the extra key bar.
//!
//! The state machine is deliberately independent of egui and of Android: positions and times come
//! in, [`Action`]s go out. That keeps the whole thing unit-testable on the host, where a tablet is
//! not available, and keeps the shell's `raw_input_hook` a thin translation layer.
//!
//! Every threshold is in egui points and seconds. Positions are clamped to the content area before
//! they are stored, so a stray coordinate from the platform can never move the pointer off-screen.

use std::collections::BTreeMap;

/// Finger movement up to this distance is still a tap, not a drag.
pub(crate) const TAP_SLOP: f32 = 10.0;
/// Two taps closer together than this and near each other form a double/triple click.
pub(crate) const DOUBLE_TAP_SECS: f64 = 0.35;
pub(crate) const DOUBLE_TAP_SLOP: f32 = 30.0;
/// Holding one finger still this long arms a drag without a preceding double tap.
pub(crate) const LONG_PRESS_SECS: f64 = 0.5;
/// Centroid movement that turns an undecided two-finger gesture into a scroll.
pub(crate) const SCROLL_SLOP: f32 = 6.0;
/// Spread change that turns an undecided two-finger gesture into a pinch.
pub(crate) const PINCH_SLOP: f32 = 12.0;
/// Both fingers have to have moved this far before a two-finger gesture is classified. Android
/// delivers one finger's move before the other's, so a scroll briefly looks like a pinch while
/// only the first finger has moved; waiting for both avoids zooming on every two-finger scroll.
pub(crate) const CLASSIFY_BOTH: f32 = 4.0;
/// If only one finger is moving (a one-sided pinch), classify after this long.
pub(crate) const CLASSIFY_DELAY_SECS: f64 = 0.12;
/// A three-finger swipe down by this much toggles the key bar.
pub(crate) const SWIPE_DOWN: f32 = 70.0;
/// Pointer travel per finger travel. Slightly above 1 so the far side of a tablet is reachable
/// without lifting the finger, while still feeling direct.
pub(crate) const POINTER_GAIN: f32 = 1.15;

/// A point in egui points, relative to the content area's top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub(crate) const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn scale(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }

    fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    fn distance(self, other: Self) -> f32 {
        (self - other).length()
    }
}

impl std::ops::Add for Point {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

/// The content area the virtual pointer may move in. A `Bounds` with inverted or non-finite
/// corners is normalised when it is set, so [`Bounds::clamp`] never panics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bounds {
    pub min: Point,
    pub max: Point,
}

impl Bounds {
    pub(crate) fn new(min: Point, max: Point) -> Self {
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let (ax, bx) = (finite(min.x), finite(max.x));
        let (ay, by) = (finite(min.y), finite(max.y));
        Self { min: Point::new(ax.min(bx), ay.min(by)), max: Point::new(ax.max(bx), ay.max(by)) }
    }

    pub(crate) fn center(self) -> Point {
        Point::new((self.min.x + self.max.x) * 0.5, (self.min.y + self.max.y) * 0.5)
    }

    // A clamp would panic if a directly constructed `Bounds` ever had min > max; this order cannot.
    #[allow(clippy::manual_clamp)]
    fn clamp(self, point: Point) -> Point {
        let x = if point.x.is_finite() { point.x } else { self.min.x };
        let y = if point.y.is_finite() { point.y } else { self.min.y };
        Point::new(x.max(self.min.x).min(self.max.x), y.max(self.min.y).min(self.max.y))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

/// One platform touch event. `id` is stable for the lifetime of a finger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchSample {
    pub id: u64,
    pub phase: TouchPhase,
    pub pos: Point,
    pub time: f64,
}

impl TouchSample {
    pub(crate) fn new(id: u64, phase: TouchPhase, pos: Point, time: f64) -> Self {
        Self { id, phase, pos, time }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollPhase {
    Move,
    End,
}

/// What the shell has to do for one gesture step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    /// Put the visible pointer at `pos` (touchpad motion, or the centre of a new multi-finger
    /// gesture so scroll and zoom act under the fingers).
    MovePointer {
        pos: Point,
    },
    Press {
        button: Button,
        pos: Point,
    },
    Release {
        button: Button,
        pos: Point,
    },
    Click {
        button: Button,
        pos: Point,
    },
    /// `delta` is in the direction the *content* should move, like an egui `MouseWheel` event.
    Scroll {
        delta: Point,
        phase: ScrollPhase,
    },
    Zoom {
        factor: f32,
    },
    ToggleKeyBar,
}

#[derive(Clone, Copy, Debug)]
struct Touch {
    start: Point,
    pos: Point,
    start_time: f64,
    moved: f32,
}

impl Touch {
    fn new(pos: Point, time: f64) -> Self {
        Self { start: pos, pos, start_time: time, moved: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Idle,
    One,
    Two,
    Three,
    /// A multi-finger gesture ended but some fingers are still down. Ignore them until all lift,
    /// so a lifted finger can never turn into a ghost click.
    Ignore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TwoMode {
    Undecided,
    Scroll,
    Pinch,
}

#[derive(Clone, Copy, Debug)]
struct TwoState {
    start_centroid: Point,
    last_centroid: Point,
    start_spread: f32,
    last_spread: f32,
    started: f64,
    mode: TwoMode,
}

#[derive(Clone, Copy, Debug)]
struct ThreeState {
    start_centroid: Point,
    toggled: bool,
}

/// The zoom factor between two finger spreads, if it is large enough to be a real gesture. A zero
/// or non-finite spread (fingers on top of each other) is ignored rather than dividing.
fn pinch_factor(from: f32, to: f32) -> Option<f32> {
    if from <= 1.0 || to <= 1.0 {
        return None;
    }
    let factor = to / from;
    if factor.is_finite() && factor > 0.0 && (factor - 1.0).abs() > 0.005 { Some(factor.clamp(0.5, 2.0)) } else { None }
}

/// Two fingers moving by `delta` scroll the page the other way, as a touchpad does.
fn scroll_by(delta: Point, out: &mut Vec<Action>) {
    if delta.length() > 0.0 {
        out.push(Action::Scroll { delta: Point::new(-delta.x, -delta.y), phase: ScrollPhase::Move });
    }
}

/// The gesture recogniser. Feed it every touch event in order, call [`Self::tick`] once a frame,
/// and apply the returned [`Action`]s to egui.
pub(crate) struct Touchpad {
    bounds: Bounds,
    pointer: Point,
    initialized: bool,
    mode: Mode,
    touches: BTreeMap<u64, Touch>,
    last_tap: Option<(f64, Point)>,
    drag_armed: bool,
    long_press: bool,
    drag: Option<Button>,
    two: Option<TwoState>,
    three: Option<ThreeState>,
}

impl Touchpad {
    pub(crate) fn new(bounds: Bounds) -> Self {
        Self {
            bounds,
            pointer: bounds.center(),
            initialized: false,
            mode: Mode::Idle,
            touches: BTreeMap::new(),
            last_tap: None,
            drag_armed: false,
            long_press: false,
            drag: None,
            two: None,
            three: None,
        }
    }

    pub(crate) fn set_bounds(&mut self, bounds: Bounds) {
        self.bounds = bounds;
        self.pointer = bounds.clamp(self.pointer);
    }

    pub(crate) fn pointer(&self) -> Point {
        self.pointer
    }

    /// Arms a long press when the finger has rested long enough. No action is emitted: the press
    /// happens when the finger starts to move, so holding still never selects text under the
    /// finger or starts a drag by accident.
    pub(crate) fn tick(&mut self, now: f64) {
        if self.mode != Mode::One || self.long_press || self.drag.is_some() || self.touches.len() != 1 {
            return;
        }
        let Some(touch) = self.touches.values().next() else {
            return;
        };
        if touch.moved <= TAP_SLOP && now - touch.start_time >= LONG_PRESS_SECS {
            self.long_press = true;
        }
    }

    pub(crate) fn process(&mut self, sample: TouchSample) -> Vec<Action> {
        match sample.phase {
            TouchPhase::Start => self.start(sample),
            TouchPhase::Move => self.moved(sample),
            TouchPhase::End => self.end(sample),
            TouchPhase::Cancel => self.cancel(),
        }
    }

    fn start(&mut self, sample: TouchSample) -> Vec<Action> {
        let mut out = Vec::new();
        let pos = self.bounds.clamp(sample.pos);
        // In `Ignore` the new finger is tracked (so `End` can remove it) but can never begin a
        // gesture of its own before the old gesture's fingers have all lifted.
        self.touches.insert(sample.id, Touch::new(pos, sample.time));
        if self.mode == Mode::Ignore {
            return out;
        }

        match self.touches.len() {
            1 => {
                self.mode = Mode::One;
                self.drag_armed =
                    self.last_tap.is_some_and(|(time, tap)| sample.time - time <= DOUBLE_TAP_SECS && tap.distance(pos) <= DOUBLE_TAP_SLOP);
                self.long_press = false;
                self.drag = None;
                self.two = None;
                self.three = None;
                // The user's first touch should act where the finger is, not in the middle of the
                // screen. After that the pointer stays where it was left, like a touchpad.
                if !self.initialized {
                    self.initialized = true;
                    self.pointer = pos;
                    out.push(Action::MovePointer { pos: self.pointer });
                }
            }
            2 => {
                self.end_drag(&mut out);
                self.last_tap = None;
                self.mode = Mode::Two;
                self.two = None;
                let (centroid, spread) = self.centroid_spread();
                self.pointer = self.bounds.clamp(centroid);
                out.push(Action::MovePointer { pos: self.pointer });
                self.two = Some(TwoState {
                    start_centroid: centroid,
                    last_centroid: centroid,
                    start_spread: spread,
                    last_spread: spread,
                    started: sample.time,
                    mode: TwoMode::Undecided,
                });
            }
            3 => {
                self.end_drag(&mut out);
                // A third finger during a two-finger scroll stops the coast before the gesture
                // becomes a three-finger tap/swipe.
                self.finish_two(&mut out);
                self.last_tap = None;
                self.mode = Mode::Three;
                self.three = None;
                let (centroid, _) = self.centroid_spread();
                self.pointer = self.bounds.clamp(centroid);
                out.push(Action::MovePointer { pos: self.pointer });
                self.three = Some(ThreeState { start_centroid: centroid, toggled: false });
            }
            _ => {
                // Four or more fingers: not a WordCraft gesture. Release anything held and ignore
                // every finger until they all lift.
                self.end_drag(&mut out);
                self.last_tap = None;
                self.finish_two(&mut out);
                self.mode = Mode::Ignore;
                self.two = None;
                self.three = None;
            }
        }
        out
    }

    fn moved(&mut self, sample: TouchSample) -> Vec<Action> {
        let mut out = Vec::new();
        let (previous, pos, moved) = {
            let Some(touch) = self.touches.get_mut(&sample.id) else {
                return out;
            };
            let previous = touch.pos;
            let pos = self.bounds.clamp(sample.pos);
            touch.pos = pos;
            touch.moved = touch.moved.max(touch.start.distance(pos));
            (previous, pos, touch.moved)
        };

        match self.mode {
            Mode::One if self.touches.len() == 1 => self.moved_one(previous, pos, moved, &mut out),
            Mode::Two => self.moved_two(sample.time, &mut out),
            Mode::Three => self.moved_three(&mut out),
            // `Cancel` clears everything; a move without a known touch id is stale.
            Mode::Idle | Mode::Ignore => {}
        }
        out
    }

    fn moved_one(&mut self, previous: Point, pos: Point, moved: f32, out: &mut Vec<Action>) {
        // A double tap followed by movement, or a long press followed by movement, is a text
        // selection drag. Press before moving so the press lands where the pointer is now, then
        // move the pointer under the finger.
        if self.drag.is_none() && moved > TAP_SLOP && (self.drag_armed || self.long_press) {
            let button = Button::Left;
            out.push(Action::Press { button, pos: self.pointer });
            self.drag = Some(button);
        }
        let delta = pos - previous;
        if delta.length() > 0.0 {
            self.pointer = self.bounds.clamp(self.pointer + delta.scale(POINTER_GAIN));
            out.push(Action::MovePointer { pos: self.pointer });
        }
    }

    fn moved_two(&mut self, time: f64, out: &mut Vec<Action>) {
        let (centroid, spread) = self.centroid_spread();
        let Some(mode) = self.two.as_ref().map(|two| two.mode) else {
            return;
        };
        match mode {
            TwoMode::Undecided => self.classify_two(time, centroid, spread, out),
            TwoMode::Scroll => self.scroll_two(centroid, out),
            TwoMode::Pinch => self.pinch_two(centroid, spread, out),
        }
    }

    fn classify_two(&mut self, time: f64, centroid: Point, spread: f32, out: &mut Vec<Action>) {
        let Some(two) = self.two.as_mut() else {
            return;
        };
        let travel = centroid.distance(two.start_centroid);
        let spread_change = (spread - two.start_spread).abs();
        let both_moved = self.touches.values().all(|touch| touch.moved >= CLASSIFY_BOTH);
        let waited = time - two.started >= CLASSIFY_DELAY_SECS;
        let pinch = two.start_spread > 1.0 && spread_change > PINCH_SLOP && (both_moved || waited);
        if pinch {
            two.mode = TwoMode::Pinch;
            two.last_spread = spread;
            two.last_centroid = centroid;
            self.pointer = self.bounds.clamp(centroid);
            out.push(Action::MovePointer { pos: self.pointer });
            if let Some(factor) = pinch_factor(two.start_spread, spread) {
                out.push(Action::Zoom { factor });
            }
        } else if travel > SCROLL_SLOP && (both_moved || waited) {
            two.mode = TwoMode::Scroll;
            two.last_centroid = centroid;
            self.pointer = self.bounds.clamp(centroid);
            out.push(Action::MovePointer { pos: self.pointer });
            scroll_by(centroid - two.start_centroid, out);
        }
    }

    fn scroll_two(&mut self, centroid: Point, out: &mut Vec<Action>) {
        let Some(two) = self.two.as_mut() else {
            return;
        };
        let delta = centroid - two.last_centroid;
        two.last_centroid = centroid;
        self.pointer = self.bounds.clamp(centroid);
        out.push(Action::MovePointer { pos: self.pointer });
        scroll_by(delta, out);
    }

    fn pinch_two(&mut self, centroid: Point, spread: f32, out: &mut Vec<Action>) {
        let Some(two) = self.two.as_mut() else {
            return;
        };
        if let Some(factor) = pinch_factor(two.last_spread, spread) {
            two.last_spread = spread;
            out.push(Action::Zoom { factor });
        }
        self.pointer = self.bounds.clamp(centroid);
        out.push(Action::MovePointer { pos: self.pointer });
    }

    fn moved_three(&mut self, out: &mut Vec<Action>) {
        let (centroid, _) = self.centroid_spread();
        let Some(three) = self.three.as_mut() else {
            return;
        };
        if three.toggled {
            return;
        }
        let total = centroid - three.start_centroid;
        if total.y > SWIPE_DOWN && total.y.abs() > total.x.abs() * 1.5 {
            three.toggled = true;
            out.push(Action::ToggleKeyBar);
        }
    }

    fn end(&mut self, sample: TouchSample) -> Vec<Action> {
        let mut out = Vec::new();
        let Some(touch) = self.touches.remove(&sample.id) else {
            return out;
        };
        let pos = self.bounds.clamp(sample.pos);
        let moved = touch.moved.max(touch.start.distance(pos));

        match self.mode {
            Mode::Idle | Mode::Ignore => {
                if self.touches.is_empty() {
                    self.mode = Mode::Idle;
                }
            }
            Mode::One => {
                if let Some(button) = self.drag.take() {
                    out.push(Action::Release { button, pos: self.pointer });
                    self.last_tap = None;
                } else if moved <= TAP_SLOP {
                    // A release that never moved is a click, however long it was held. A long
                    // hold that moved became a drag above; a swipe that never armed a drag only
                    // moved the pointer and must not click.
                    out.push(Action::Click { button: Button::Left, pos: self.pointer });
                    // Remember this tap so a nearby second one becomes a double click and a
                    // second tap followed by movement becomes a drag.
                    self.last_tap = Some((sample.time, touch.start));
                } else {
                    self.last_tap = None;
                }
                self.drag_armed = false;
                self.long_press = false;
                self.mode = if self.touches.is_empty() { Mode::Idle } else { Mode::Ignore };
            }
            Mode::Two => {
                if let Some(two) = self.two.take() {
                    match two.mode {
                        TwoMode::Scroll => {
                            // Tells the canvas the fingers lifted, which starts its momentum
                            // coast instead of waiting for an idle timeout.
                            out.push(Action::Scroll { delta: Point::ZERO, phase: ScrollPhase::End });
                        }
                        TwoMode::Undecided => {
                            let moved = self.touches.values().map(|touch| touch.moved).fold(moved, f32::max);
                            if moved <= TAP_SLOP {
                                out.push(Action::Click { button: Button::Right, pos: self.pointer });
                            }
                        }
                        TwoMode::Pinch => {}
                    }
                }
                self.mode = if self.touches.is_empty() { Mode::Idle } else { Mode::Ignore };
            }
            Mode::Three => {
                if let Some(three) = self.three.take() {
                    let moved = self.touches.values().map(|touch| touch.moved).fold(moved, f32::max);
                    if !three.toggled && moved <= TAP_SLOP {
                        out.push(Action::Click { button: Button::Middle, pos: self.pointer });
                    }
                }
                self.mode = if self.touches.is_empty() { Mode::Idle } else { Mode::Ignore };
            }
        }
        out
    }

    fn cancel(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(button) = self.drag.take() {
            out.push(Action::Release { button, pos: self.pointer });
        }
        if self.two.as_ref().is_some_and(|two| two.mode == TwoMode::Scroll) {
            out.push(Action::Scroll { delta: Point::ZERO, phase: ScrollPhase::End });
        }
        self.touches.clear();
        self.mode = Mode::Idle;
        self.last_tap = None;
        self.drag_armed = false;
        self.long_press = false;
        self.two = None;
        self.three = None;
        out
    }

    fn end_drag(&mut self, out: &mut Vec<Action>) {
        if let Some(button) = self.drag.take() {
            out.push(Action::Release { button, pos: self.pointer });
        }
        self.drag_armed = false;
        self.long_press = false;
    }

    fn finish_two(&mut self, out: &mut Vec<Action>) {
        if let Some(two) = self.two.take()
            && two.mode == TwoMode::Scroll
        {
            out.push(Action::Scroll { delta: Point::ZERO, phase: ScrollPhase::End });
        }
    }

    fn centroid_spread(&self) -> (Point, f32) {
        let count = self.touches.len();
        if count == 0 {
            return (Point::ZERO, 0.0);
        }
        let mut sum = Point::ZERO;
        for touch in self.touches.values() {
            sum.x += touch.pos.x;
            sum.y += touch.pos.y;
        }
        let centroid = Point::new(sum.x / count as f32, sum.y / count as f32);
        let mut iter = self.touches.values();
        let spread = match (iter.next(), iter.next()) {
            (Some(a), Some(b)) => a.pos.distance(b.pos),
            _ => 0.0,
        };
        (centroid, spread)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Bounds {
        Bounds::new(Point::new(0.0, 0.0), Point::new(1000.0, 1000.0))
    }

    fn start(pad: &mut Touchpad, id: u64, x: f32, y: f32, t: f64) -> Vec<Action> {
        pad.process(TouchSample::new(id, TouchPhase::Start, Point::new(x, y), t))
    }

    fn moved(pad: &mut Touchpad, id: u64, x: f32, y: f32, t: f64) -> Vec<Action> {
        pad.process(TouchSample::new(id, TouchPhase::Move, Point::new(x, y), t))
    }

    fn end(pad: &mut Touchpad, id: u64, x: f32, y: f32, t: f64) -> Vec<Action> {
        pad.process(TouchSample::new(id, TouchPhase::End, Point::new(x, y), t))
    }

    fn approx(actual: Point, expected: Point) {
        assert!((actual.x - expected.x).abs() < 0.05 && (actual.y - expected.y).abs() < 0.05, "{actual:?} != {expected:?}");
    }

    fn clicks(actions: &[Action]) -> Vec<Button> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Click { button, .. } => Some(*button),
                _ => None,
            })
            .collect()
    }

    fn presses(actions: &[Action]) -> Vec<Button> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Press { button, .. } => Some(*button),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn first_touch_places_the_pointer_where_the_finger_is_and_taps_click() {
        let mut pad = Touchpad::new(bounds());
        assert_eq!(pad.pointer(), Point::new(500.0, 500.0));
        let actions = start(&mut pad, 1, 100.0, 200.0, 0.0);
        assert_eq!(actions, vec![Action::MovePointer { pos: Point::new(100.0, 200.0) }]);
        let actions = end(&mut pad, 1, 100.0, 200.0, 0.1);
        assert_eq!(actions, vec![Action::Click { button: Button::Left, pos: Point::new(100.0, 200.0) }]);
    }

    #[test]
    fn one_finger_swipe_moves_the_pointer_without_selecting() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 100.0, 100.0, 0.0);
        let actions = moved(&mut pad, 1, 200.0, 100.0, 0.05);
        assert!(presses(&actions).is_empty(), "a plain swipe must not press");
        let pointer = pad.pointer();
        approx(pointer, Point::new(215.0, 100.0)); // 100 * 1.15
        let actions = end(&mut pad, 1, 200.0, 100.0, 0.1);
        assert!(clicks(&actions).is_empty(), "a swipe is not a tap");
    }

    #[test]
    fn double_tap_then_drag_presses_moves_and_releases() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 100.0, 100.0, 0.0);
        assert_eq!(clicks(&end(&mut pad, 1, 100.0, 100.0, 0.1)), vec![Button::Left]);
        start(&mut pad, 2, 103.0, 102.0, 0.2);
        let actions = moved(&mut pad, 2, 160.0, 102.0, 0.25);
        assert_eq!(presses(&actions), vec![Button::Left]);
        assert!(actions.windows(2).any(|pair| matches!(pair, [Action::Press { .. }, Action::MovePointer { .. }])));
        let actions = end(&mut pad, 2, 160.0, 102.0, 0.3);
        assert!(matches!(actions.as_slice(), [Action::Release { button: Button::Left, .. }]));
        assert!(clicks(&actions).is_empty(), "ending a drag must not also click");
    }

    #[test]
    fn long_press_then_drag_presses_without_a_preceding_tap() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 400.0, 400.0, 0.0);
        pad.tick(LONG_PRESS_SECS + 0.01);
        let actions = moved(&mut pad, 1, 440.0, 400.0, LONG_PRESS_SECS + 0.02);
        assert_eq!(presses(&actions), vec![Button::Left]);
    }

    #[test]
    fn a_long_still_hold_clicks_on_release_without_pressing_first() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 300.0, 300.0, 0.0);
        pad.tick(LONG_PRESS_SECS + 0.1);
        let actions = end(&mut pad, 1, 300.0, 300.0, LONG_PRESS_SECS + 0.2);
        assert!(presses(&actions).is_empty(), "a hold that never moved must not start a drag");
        assert_eq!(clicks(&actions), vec![Button::Left]);
    }

    #[test]
    fn a_second_finger_cancels_a_drag_and_never_clicks() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 100.0, 100.0, 0.0);
        end(&mut pad, 1, 100.0, 100.0, 0.05);
        start(&mut pad, 2, 100.0, 100.0, 0.1);
        moved(&mut pad, 2, 160.0, 100.0, 0.15);
        let actions = start(&mut pad, 3, 300.0, 300.0, 0.2);
        assert!(matches!(actions.as_slice(), [Action::Release { button: Button::Left, .. }]));
        assert!(clicks(&actions).is_empty());
    }

    #[test]
    fn two_finger_scroll_moves_content_the_other_way_and_never_selects() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 400.0, 500.0, 0.0);
        start(&mut pad, 2, 500.0, 500.0, 0.01);
        // The first finger's move alone must not be classified: Android reports the fingers one
        // after the other, and a scroll briefly looks like a pinch in between.
        let actions = moved(&mut pad, 1, 400.0, 460.0, 0.02);
        assert!(actions.iter().all(|action| !matches!(action, Action::Scroll { .. } | Action::Zoom { .. })));
        assert!(presses(&actions).is_empty());
        // Both fingers have moved up by 40 points: the content moves down by 40.
        let actions = moved(&mut pad, 2, 500.0, 460.0, 0.03);
        let scroll: Vec<Point> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Scroll { delta, phase: ScrollPhase::Move } => Some(*delta),
                _ => None,
            })
            .collect();
        assert!(scroll.iter().any(|delta| delta.y > 0.0 && delta.x.abs() < 0.01), "scroll down: {scroll:?}");
        assert!(presses(&actions).is_empty(), "two-finger scrolling must not select");
        let actions = end(&mut pad, 1, 400.0, 460.0, 0.4);
        assert!(matches!(actions.as_slice(), [Action::Scroll { phase: ScrollPhase::End, .. }]));
        let actions = end(&mut pad, 2, 500.0, 460.0, 0.41);
        assert!(clicks(&actions).is_empty(), "the remaining finger must not turn into a click");
    }

    #[test]
    fn two_finger_tap_right_clicks_once() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 400.0, 400.0, 0.0);
        start(&mut pad, 2, 500.0, 400.0, 0.01);
        let actions = end(&mut pad, 1, 400.0, 400.0, 0.1);
        assert_eq!(clicks(&actions), vec![Button::Right]);
        let actions = end(&mut pad, 2, 500.0, 400.0, 0.11);
        assert!(clicks(&actions).is_empty(), "one right-click per gesture");
    }

    #[test]
    fn pinch_zooms_around_the_fingers() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 400.0, 400.0, 0.0);
        start(&mut pad, 2, 600.0, 400.0, 0.01);
        moved(&mut pad, 1, 350.0, 400.0, 0.02);
        let actions = moved(&mut pad, 2, 650.0, 400.0, 0.03);
        let factor: f32 = actions
            .iter()
            .filter_map(|action| match action {
                Action::Zoom { factor } => Some(*factor),
                _ => None,
            })
            .product();
        assert!((factor - 1.5).abs() < 0.1, "pinch spread 200 -> 300: {factor}");
        assert!(presses(&actions).is_empty());
        approx(pad.pointer(), Point::new(500.0, 400.0));
    }

    #[test]
    fn three_finger_tap_middle_clicks_once() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 300.0, 300.0, 0.0);
        start(&mut pad, 2, 400.0, 300.0, 0.01);
        start(&mut pad, 3, 500.0, 300.0, 0.02);
        let actions = end(&mut pad, 1, 300.0, 300.0, 0.1);
        assert_eq!(clicks(&actions), vec![Button::Middle]);
        assert!(clicks(&end(&mut pad, 2, 400.0, 300.0, 0.11)).is_empty());
        assert!(clicks(&end(&mut pad, 3, 500.0, 300.0, 0.12)).is_empty());
    }

    #[test]
    fn three_finger_swipe_down_toggles_the_key_bar_once() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 300.0, 300.0, 0.0);
        start(&mut pad, 2, 400.0, 300.0, 0.01);
        start(&mut pad, 3, 500.0, 300.0, 0.02);
        let mut toggles = 0;
        for y in [320.0, 350.0, 390.0, 420.0] {
            moved(&mut pad, 1, 300.0, y, 0.1);
            moved(&mut pad, 2, 400.0, y, 0.11);
            for action in moved(&mut pad, 3, 500.0, y, 0.12) {
                if action == Action::ToggleKeyBar {
                    toggles += 1;
                }
            }
        }
        assert_eq!(toggles, 1);
        assert!(clicks(&end(&mut pad, 3, 500.0, 420.0, 0.3)).is_empty());
    }

    #[test]
    fn four_fingers_are_ignored_and_never_click() {
        let mut pad = Touchpad::new(bounds());
        for id in 1..=4 {
            start(&mut pad, id, 200.0 + id as f32 * 50.0, 300.0, 0.0);
        }
        for id in 1..=4 {
            let actions = end(&mut pad, id, 200.0 + id as f32 * 50.0, 300.0, 0.1);
            assert!(clicks(&actions).is_empty());
        }
    }

    #[test]
    fn cancel_releases_a_drag_and_never_clicks() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 100.0, 100.0, 0.0);
        end(&mut pad, 1, 100.0, 100.0, 0.05);
        start(&mut pad, 2, 100.0, 100.0, 0.1);
        moved(&mut pad, 2, 160.0, 100.0, 0.15);
        let actions = pad.process(TouchSample::new(2, TouchPhase::Cancel, Point::new(160.0, 100.0), 0.2));
        assert!(matches!(actions.as_slice(), [Action::Release { button: Button::Left, .. }]));
    }

    #[test]
    fn pointer_stays_inside_the_content_area() {
        let mut pad = Touchpad::new(bounds());
        start(&mut pad, 1, 100.0, 100.0, 0.0);
        for _ in 0..20 {
            moved(&mut pad, 1, 900.0, 900.0, 0.1);
        }
        let pointer = pad.pointer();
        assert!(pointer.x <= 1000.0 && pointer.y <= 1000.0, "{pointer:?}");
        pad.set_bounds(Bounds::new(Point::new(0.0, 0.0), Point::new(200.0, 200.0)));
        let pointer = pad.pointer();
        assert!(pointer.x <= 200.0 && pointer.y <= 200.0, "{pointer:?}");
    }

    #[test]
    fn hostile_bounds_and_positions_do_not_panic() {
        let mut pad = Touchpad::new(Bounds::new(Point::new(f32::NAN, 10.0), Point::new(-10.0, f32::INFINITY)));
        start(&mut pad, 1, f32::NAN, f32::INFINITY, 0.0);
        moved(&mut pad, 1, f32::INFINITY, f32::NAN, f64::NAN);
        end(&mut pad, 1, f32::NAN, f32::NAN, f64::NAN);
        let pointer = pad.pointer();
        assert!(pointer.x.is_finite() && pointer.y.is_finite(), "{pointer:?}");
    }
}
