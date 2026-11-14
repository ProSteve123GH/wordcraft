use cfg_aliases::cfg_aliases;

fn main() {
    // The script doesn't depend on our code.
    println!("cargo:rerun-if-changed=build.rs");

    // Setup cfg aliases.
    cfg_aliases! {
        // Systems.
        // Termux port: Android ships no winit backend we can drive from a plain
        // executable, so an X11 build treats Android like Linux.
        android_platform: { all(target_os = "android", not(feature = "x11")) },
        web_platform: { all(target_family = "wasm", target_os = "unknown") },
        macos_platform: { target_os = "macos" },
        ios_platform: { target_os = "ios" },
        windows_platform: { target_os = "windows" },
        apple: { any(target_os = "ios", target_os = "macos") },
        // Termux port: no `not(android_platform)` here, so x11_platform/wayland_platform
        // become available on Android when those features are on.
        free_unix: { all(unix, not(apple), not(target_os = "emscripten")) },
        redox: { target_os = "redox" },

        // Native displays.
        x11_platform: { all(feature = "x11", free_unix, not(redox)) },
        wayland_platform: { all(feature = "wayland", free_unix, not(redox)) },
        orbital_platform: { redox },
    }

    // Winit defined cfgs.
    println!("cargo:rustc-check-cfg=cfg(unreleased_changelogs)");
}
