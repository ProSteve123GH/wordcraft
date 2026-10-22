package com.storytold.wordcraft;

import com.google.androidgamesdk.GameActivity;

/**
 * WordCraft's Android activity.
 *
 * <p>GameActivity owns the SurfaceView, input dispatch, IME plumbing and the native lifecycle
 * callbacks. Rust receives the activity through {@code android_main}. A later milestone can add
 * SAF, clipboard and IME bridges here, but M-A0 deliberately keeps the Java shell empty.</p>
 */
public final class MainActivity extends GameActivity {
}
