//! The bridge between the Java `MainActivity` and the Rust shell.
//!
//! winit's Android backend has no Back or window-inset hook, and a `NativeActivity` takes the
//! window's input queue, so Java `dispatchTouchEvent`/`dispatchKeyEvent` overrides never see
//! pointer or key events. `MainActivity` therefore reports only the things Java does own: the
//! editing state Back needs, window insets, input-device changes, and the IME toggle. Pointer and
//! key events arrive through winit as usual.
//!
//! The JNI functions ignore their `JNIEnv`/`JClass` arguments (they never touch the VM), so they
//! need no `jni` crate and no `unsafe` block. The symbol names encode the Java class
//! `ai.storyteller.wordcraft.MainActivity`; renaming either side means renaming the other.
//!
//! Everything except the exports also compiles on the host, so the state mapping is unit-tested
//! without an Android device.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Which physical input devices the Android `InputManager` currently reports. The built-in
/// touchscreen is not one of them: this is about adaptively supporting mice and keyboards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Devices {
    pub mouse: bool,
    pub touchpad: bool,
    pub keyboard: bool,
}

/// System bar insets, in physical pixels, reported by the Java `WindowInsets` listener. This is
/// more reliable than `NativeActivity`'s content rectangle on devices that draw edge to edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowInsets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

const MOUSE: u8 = 1;
const TOUCHPAD: u8 = 2;
const KEYBOARD: u8 = 4;

// A const `Mutex` needs no allocation and no lazy initialization, so the Java callbacks can run
// before `android_main` has set anything up.
static WINDOW_INSETS: Mutex<Option<WindowInsets>> = Mutex::new(None);
static EDITING: AtomicBool = AtomicBool::new(false);
static DEVICES: AtomicU8 = AtomicU8::new(0);
/// Set when Java reports new insets, so the shell can schedule frames until they are applied.
static INSETS_DIRTY: AtomicBool = AtomicBool::new(false);

/// The Activity handle, for the IME. `AndroidApp` is a cheap, `Send + Sync` clone and the JNI
/// callbacks run on the Activity's UI thread, so Back can show or hide the keyboard directly
/// instead of relying on `InputMethodManager.showSoftInput` attaching to the native view.
#[cfg(target_os = "android")]
static ANDROID_APP: Mutex<Option<winit::platform::android::activity::AndroidApp>> = Mutex::new(None);

fn lock<T>(mutex: &'static Mutex<T>) -> MutexGuard<'static, T> {
    // A poisoned lock only means another callback panicked; the data is still plain state, so keep
    // using it instead of unwrapping (the crate denies `unwrap`).
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub(crate) fn set_devices(mouse: bool, touchpad: bool, keyboard: bool) {
    let mut bits = 0;
    if mouse {
        bits |= MOUSE;
    }
    if touchpad {
        bits |= TOUCHPAD;
    }
    if keyboard {
        bits |= KEYBOARD;
    }
    DEVICES.store(bits, Ordering::Relaxed);
}

pub(crate) fn devices() -> Devices {
    let bits = DEVICES.load(Ordering::Relaxed);
    Devices { mouse: bits & MOUSE != 0, touchpad: bits & TOUCHPAD != 0, keyboard: bits & KEYBOARD != 0 }
}

/// Whether a text editor currently wants the keyboard. Java's Back handler reads this: while
/// editing, Back toggles the IME instead of leaving the Activity.
pub(crate) fn editing() -> bool {
    EDITING.load(Ordering::Relaxed)
}

pub(crate) fn set_editing(editing: bool) {
    EDITING.store(editing, Ordering::Relaxed);
}

pub(crate) fn set_window_insets(left: i32, top: i32, right: i32, bottom: i32) {
    *lock(&WINDOW_INSETS) = Some(WindowInsets { left: left.max(0), top: top.max(0), right: right.max(0), bottom: bottom.max(0) });
    INSETS_DIRTY.store(true, Ordering::Relaxed);
}

pub(crate) fn window_insets() -> Option<WindowInsets> {
    *lock(&WINDOW_INSETS)
}

/// Whether new insets arrived since the last check. A changed inset does not produce a winit event
/// of its own, so `watch_system_bars` asks for a few frames after Java reports one.
pub(crate) fn take_insets_dirty() -> bool {
    INSETS_DIRTY.swap(false, Ordering::Relaxed)
}

/// Keep the `AndroidApp` for the Java IME calls. Called once from `app::run` before the event loop.
#[cfg(target_os = "android")]
pub(crate) fn set_android_app(app: winit::platform::android::activity::AndroidApp) {
    *lock(&ANDROID_APP) = Some(app);
}

#[cfg(target_os = "android")]
fn with_app(action: impl FnOnce(&winit::platform::android::activity::AndroidApp)) {
    let guard = lock(&ANDROID_APP);
    if let Some(app) = guard.as_ref() {
        action(app);
    }
}

/// Show the soft keyboard using `android-activity`'s own path, which the IME is attached to. The
/// Java fallback (`InputMethodManager.showSoftInput` on the decor view) is not reliable for a
/// NativeActivity because the native window is not an `EditText`.
#[cfg(target_os = "android")]
pub(crate) fn show_soft_input() {
    with_app(|app| app.show_soft_input(true));
}

#[cfg(target_os = "android")]
pub(crate) fn hide_soft_input() {
    with_app(|app| app.hide_soft_input(false));
}

#[cfg(test)]
fn reset() {
    set_devices(false, false, false);
    set_editing(false);
    *lock(&WINDOW_INSETS) = None;
    INSETS_DIRTY.store(false, Ordering::Relaxed);
}

// The JNI entry points. `JNIEnv`/`JClass` are opaque pointers we do not touch; the primitive
// types match JNI's `jboolean` (`u8`) and `jint` (`i32`) on every ABI.
#[cfg(target_os = "android")]
mod exports {
    use super::*;
    use core::ffi::c_void;

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ai_storyteller_wordcraft_MainActivity_nativeIsEditing(_env: *mut c_void, _class: *mut c_void) -> u8 {
        u8::from(editing())
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ai_storyteller_wordcraft_MainActivity_nativeDevicesChanged(
        _env: *mut c_void,
        _class: *mut c_void,
        mouse: u8,
        touchpad: u8,
        keyboard: u8,
    ) {
        set_devices(mouse != 0, touchpad != 0, keyboard != 0);
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ai_storyteller_wordcraft_MainActivity_nativeShowKeyboard(_env: *mut c_void, _class: *mut c_void) {
        show_soft_input();
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ai_storyteller_wordcraft_MainActivity_nativeHideKeyboard(_env: *mut c_void, _class: *mut c_void) {
        hide_soft_input();
    }

    #[unsafe(no_mangle)]
    #[allow(clippy::too_many_arguments)]
    pub extern "system" fn Java_ai_storyteller_wordcraft_MainActivity_nativeWindowInsets(
        _env: *mut c_void,
        _class: *mut c_void,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    ) {
        set_window_insets(left, top, right, bottom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // This test touches process-wide state, so it is one test to avoid racing the globals
    // between parallel tests.
    #[test]
    fn devices_editing_and_insets_round_trip() {
        reset();

        set_devices(true, false, true);
        assert_eq!(devices(), Devices { mouse: true, touchpad: false, keyboard: true });

        set_editing(true);
        assert!(editing());
        set_editing(false);
        assert!(!editing());

        // Hostile/negative insets from an OEM are clamped; a dirty flag wakes the shell once.
        set_window_insets(-4, 48, 0, 64);
        assert_eq!(window_insets(), Some(WindowInsets { left: 0, top: 48, right: 0, bottom: 64 }));
        assert!(take_insets_dirty());
        assert!(!take_insets_dirty());

        reset();
        assert_eq!(window_insets(), None);
        assert_eq!(devices(), Devices::default());
        assert!(!editing());
    }
}
