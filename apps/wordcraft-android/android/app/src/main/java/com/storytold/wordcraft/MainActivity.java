package com.storytold.wordcraft;

import android.graphics.Color;
import android.os.Bundle;
import android.view.View;

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
        decor.setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                        | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                        | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                        | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_FULLSCREEN);
        getWindow().setStatusBarColor(Color.TRANSPARENT);
        getWindow().setNavigationBarColor(Color.TRANSPARENT);
    }
}
