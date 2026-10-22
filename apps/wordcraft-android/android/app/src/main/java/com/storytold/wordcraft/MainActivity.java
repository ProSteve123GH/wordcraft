package com.storytold.wordcraft;

import android.graphics.Color;
import android.os.Bundle;
import android.view.View;

import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.google.androidgamesdk.GameActivity;

/**
 * WordCraft's Android activity.
 *
 * <p>GameActivity owns the SurfaceView, input dispatch, IME plumbing and the native lifecycle
 * callbacks. Rust receives the activity through {@code android_main}. A later milestone can add
 * SAF, clipboard and IME bridges here, but M-A0 deliberately keeps the Java shell small.</p>
 *
 * <p>The document canvas is the whole tablet screen. WordCraft draws its own ribbon and page; the
 * system bars stay hidden until the user swipes them in, and the soft keyboard resizes the game
 * surface through {@code adjustResize}.</p>
 */
public final class MainActivity extends GameActivity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        hideSystemBars();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        View decor = getWindow().getDecorView();
        WindowInsetsControllerCompat controller =
                WindowCompat.getInsetsController(getWindow(), decor);
        controller.hide(WindowInsetsCompat.Type.systemBars());
        controller.setSystemBarsBehavior(
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        getWindow().setStatusBarColor(Color.TRANSPARENT);
        getWindow().setNavigationBarColor(Color.TRANSPARENT);
    }
}
