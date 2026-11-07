package ai.storyteller.wordcraft;

import android.app.NativeActivity;
import android.content.Context;
import android.graphics.Insets;
import android.hardware.input.InputManager;
import android.os.Build;
import android.os.Bundle;
import android.view.InputDevice;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.window.OnBackInvokedDispatcher;

/**
 * The Activity around the native winit/eframe loop.
 *
 * <p>NativeActivity already creates the surface and loads {@code libwordcraft.so}. This subclass
 * adds only the things winit's Android backend cannot express:
 *
 * <ul>
 *   <li>Back: while a text editor wants the keyboard, toggle the IME instead of leaving the app;
 *       otherwise preserve Android's normal Back behaviour. Android 13+ routes Back through
 *       {@code OnBackInvokedCallback} rather than {@code onBackPressed}.
 *   <li>window insets: report the status bar, cutout and navigation-bar insets, so the Rust shell
 *       can lay the interface out inside them on every device and rotation.
 *   <li>device changes: tell the shell when a physical mouse, touchpad or keyboard is attached.
 * </ul>
 *
 * <p>NativeActivity calls {@code Window.takeInputQueue}, so pointer and key events go straight to
 * the native input queue that winit reads; Java dispatch overrides would never see them. Physical
 * keyboard modifiers are therefore tracked from the key events winit delivers. Mouse hover, wheel
 * and extra buttons are discarded by winit's Android backend, which only translates touch and
 * keys (a known limitation until winit exposes them).
 *
 * <p>Every native call is a static JNI function exported by the Rust crate (src/input.rs). The
 * class name and the method names are part of the ABI; changing either side requires changing the
 * other. The library is loaded by {@link NativeActivity#onCreate}, before {@link #onCreate} calls
 * any of them.
 */
public class MainActivity extends NativeActivity implements InputManager.InputDeviceListener {
    private InputManager inputManager;
    /** Whether the soft keyboard is currently showing, from the IME window inset (API 30+). */
    private boolean imeVisible;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        // The keyboard must shrink the window, not cover the document or the extra key bar.
        // NativeActivity applies ADJUST_RESIZE by default; making it explicit survives theme
        // changes and OEM customisations.
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);

        installInsetsListener();
        installInputDeviceListener();
        installBackHandler();
    }

    /**
     * Reports the system-bar, cutout and IME insets in physical pixels. The Rust shell treats them
     * as a safe area and keeps both bars visible; nothing is drawn underneath them. The IME is
     * included because NativeActivity lays its surface out edge to edge, so on Android 11+ the
     * window does not always shrink for the keyboard and the shell must make room itself.
     */
    private void installInsetsListener() {
        final View decor = getWindow().getDecorView();
        decor.setOnApplyWindowInsetsListener(
                (view, insets) -> {
                    int left;
                    int top;
                    int right;
                    int bottom;
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        int[] bars = Api30.safeArea(insets);
                        left = bars[0];
                        top = bars[1];
                        right = bars[2];
                        bottom = bars[3];
                        imeVisible = Api30.imeVisible(insets);
                    } else {
                        left = insets.getSystemWindowInsetLeft();
                        top = insets.getSystemWindowInsetTop();
                        right = insets.getSystemWindowInsetRight();
                        bottom = insets.getSystemWindowInsetBottom();
                    }
                    nativeWindowInsets(left, top, right, bottom);
                    // Do not consume the insets: the view hierarchy (and the IME resize) still
                    // needs them. We only report them.
                    return insets;
                });
        decor.requestApplyInsets();
    }

    private void installInputDeviceListener() {
        inputManager = (InputManager) getSystemService(Context.INPUT_SERVICE);
        if (inputManager != null) {
            inputManager.registerInputDeviceListener(this, null);
        }
        reportInputDevices();
    }

    /** Counts physical, non-virtual mice, touchpads and alphabetic keyboards. */
    private void reportInputDevices() {
        boolean mouse = false;
        boolean touchpad = false;
        boolean keyboard = false;
        if (inputManager != null) {
            for (int id : inputManager.getInputDeviceIds()) {
                InputDevice device = inputManager.getInputDevice(id);
                if (device == null || device.isVirtual()) {
                    continue;
                }
                int sources = device.getSources();
                if ((sources & InputDevice.SOURCE_MOUSE) == InputDevice.SOURCE_MOUSE) {
                    mouse = true;
                }
                if ((sources & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD) {
                    touchpad = true;
                }
                if ((sources & InputDevice.SOURCE_KEYBOARD) == InputDevice.SOURCE_KEYBOARD
                        && device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC) {
                    keyboard = true;
                }
            }
        }
        nativeDevicesChanged(mouse, touchpad, keyboard);
    }

    @Override
    public void onInputDeviceAdded(int deviceId) {
        reportInputDevices();
    }

    @Override
    public void onInputDeviceRemoved(int deviceId) {
        reportInputDevices();
    }

    @Override
    public void onInputDeviceChanged(int deviceId) {
        reportInputDevices();
    }

    @Override
    protected void onDestroy() {
        if (inputManager != null) {
            inputManager.unregisterInputDeviceListener(this);
            inputManager = null;
        }
        super.onDestroy();
    }

    /**
     * Android 13+ (target 33+) routes Back through {@code OnBackInvokedCallback} instead of the
     * deprecated {@code onBackPressed}. Registering the dispatcher keeps the IME toggle working
     * with gesture and three-button navigation; older releases use the override below.
     */
    private void installBackHandler() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Api33.register(this);
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        handleBack();
    }

    private void handleBack() {
        boolean keyboardVisible = imeVisible;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            // Before API 30 there is no IME visibility query; this is the best available signal.
            InputMethodManager imm =
                    (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            keyboardVisible = imm != null && imm.isAcceptingText();
        }
        if (keyboardVisible) {
            // The keyboard is visible: Back hides it and stays in the document. Go through the
            // native side so this uses the same window that winit's IME is attached to.
            nativeHideKeyboard();
            return;
        }
        if (nativeIsEditing()) {
            // A text editor still has focus but the keyboard was dismissed: Back brings it back.
            nativeShowKeyboard();
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            // The system already ran the predictive-back animation. Calling super.onBackPressed()
            // here would re-enter the dispatcher and this callback, so finish directly.
            finishAfterTransition();
        } else {
            super.onBackPressed();
        }
    }

    /**
     * The API 30 inset API lives in its own class for the same reason as {@link Api33}: the
     * {@code android.window} and {@code WindowInsets.Type} references must not be resolved while
     * loading {@code MainActivity} on Android 8 or 9, where they do not exist yet.
     */
    private static final class Api30 {
        static int[] safeArea(WindowInsets insets) {
            Insets bars =
                    insets.getInsets(
                            WindowInsets.Type.systemBars()
                                    | WindowInsets.Type.displayCutout()
                                    | WindowInsets.Type.ime());
            return new int[] {bars.left, bars.top, bars.right, bars.bottom};
        }

        static boolean imeVisible(WindowInsets insets) {
            return insets.isVisible(WindowInsets.Type.ime());
        }
    }

    /**
     * The API 33 back callback lives in its own class so older Android versions never verify a
     * reference to {@code android.window} while loading MainActivity.
     */
    private static final class Api33 {
        static void register(MainActivity activity) {
            activity.getOnBackInvokedDispatcher()
                    .registerOnBackInvokedCallback(
                            OnBackInvokedDispatcher.PRIORITY_DEFAULT, activity::handleBack);
        }
    }

    // Implemented in src/input.rs, loaded with libwordcraft.so by NativeActivity.
    private static native boolean nativeIsEditing();

    private static native void nativeShowKeyboard();

    private static native void nativeHideKeyboard();

    private static native void nativeDevicesChanged(
            boolean mouse, boolean touchpad, boolean keyboard);

    private static native void nativeWindowInsets(int left, int top, int right, int bottom);
}
