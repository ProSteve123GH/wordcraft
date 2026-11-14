# WordCraft on Termux (native Android, X11, GPU accelerated)

WordCraft normally targets macOS, Windows, Linux and the BSDs. This is a port that runs the real
desktop build **natively in Termux** (no proot, no chroot, no VNC) on Termux:X11, drawing through
the device's GPU with VirGL.

Status: builds with Termux's `rust` package (`aarch64-linux-android`, `target_env = ""`); the UI
runs on egui/eframe with wgpu's OpenGL backend, over Mesa's `virpipe` driver (VirGL), i.e. on the
Mali GPU rather than llvmpipe. See "Known limits" below for what is not available.

## Why Android needed patches

Termux's Rust target is `aarch64-linux-android`, so `cfg(target_os = "android")` is true. The GUI
stack treats "unix desktop" as "Linux", and several crates either skip their unix backend on
Android or select an Android-only backend that needs an `android_main` activity. The port keeps
the real X11 code by vendoring a few crates and telling them Android is a unix desktop
(see `vendor/README.md` for the exact edits):

| Crate | Change |
|---|---|
| `winit` | X11 backend on Android; `android_platform` is off when `x11` is on |
| `eframe` | desktop event loop instead of the Android activity one; quits as soon as the accepted close drops the last window |
| `egui-winit` | arboard clipboard on Android |
| `arboard`, `rfd` | Linux backend on Android |
| `accesskit_winit` | no `winit/wayland` in its default features (Wayland is not used here) |
| `x11rb-protocol` | X11 socket directory from `$PREFIX/tmp/.X11-unix` (Android has no writable `/tmp`), so the clipboard can connect |

`apps/wordcraft/Cargo.toml` also drops eframe's `wayland` feature: this port runs X11 only, and on
Android the Wayland path cannot build (`wayland-cursor` needs `rustix::shm`, which rustix does not
provide for Android).

## Prerequisites

```bash
pkg install rust binutils pkg-config fontconfig cmake make
pkg install x11-repo
pkg install termux-x11-nightly mesa virglrenderer-android \
            libx11 libxcb libxkbcommon libxrandr libxi libxcursor \
            libxrender libxext libxfixes libxau libxdmcp
```

(`cmake`/`make` are only needed if a C dependency ever has to be built.)

Hardware acceleration in Termux on this tablet:

```bash
virgl_test_server_android &     # exposes the phone's real GLES driver
GALLIUM_DRIVER=virpipe glxinfo -B | grep renderer   # -> virgl (Mali-G615 MC2)
```

## Build

```bash
bash ~/wordcraft/build-termux.sh
```

The script sets two compiler jobs, no debug info and `nice`, and watches `MemAvailable`: if Android
gets close to the low-memory killer it pauses the compilers for a moment so the kernel can reclaim
memory. This is a debug build on purpose: a release build with `lto = "thin"` needs far more memory
than this device has available while other apps are running.

## Run

```bash
bash ~/wordcraft/run-termux.sh --sample
```

It starts `virgl_test_server_android` and `termux-x11 :0` if they are not running, exports
`WGPU_BACKEND=gl`, `WINIT_UNIX_BACKEND=x11`, `WORDCRAFT_WAYLAND=0` and
`WORDCRAFT_FILE_DIALOG=builtin`, then starts the app. The Termux:X11 app must be open on screen.
Both servers start detached, so closing the terminal they were launched from leaves the display up.
A crashed X server leaves a stale `$PREFIX/tmp/.X11-unix/X0` behind; the script checks for the
process and clears the socket, so a dead session restarts instead of failing with
`XOpenDisplayFailed`.

Closing the window quits the app, saving preferences first. (eframe normally waits for one more
window event after the last window is dropped before it exits the loop; with no window left there
may be none, so the patched copy exits straight away.)

## Following the desktop theme

File › Options › General › Interface theme has Light, Dark and a system-following choice; the
control method is `ui.theme` with `light`, `dark` or `system`. On a Linux desktop WordCraft asks the
XDG desktop portal. Termux has no portal and no session bus, so on Android it reads the X11 side
directly, in this order:

1. **XSETTINGS**, published by the running settings manager (`xfsettingsd` in XFCE): the manager
   owns the `_XSETTINGS_S0` selection and its `Net/ThemeName` contains `dark` (XFCE names its dark
   variants that way: `Adwaita-dark`, `Greybird-dark`…), or `Gtk/ApplicationPreferDarkTheme` is on.
2. **GTK's `settings.ini`**, `~/.config/gtk-4.0/settings.ini` then `gtk-3.0` (`gtk-theme-name`,
   `gtk-application-prefer-dark-theme`), for when no settings manager is running.
3. **XFCE's stored channel**, `~/.config/xfce4/xfconf/xfce-perchannel-xml/xsettings.xml`
   (`/Net/ThemeName`), which `xfconf-query` writes within a couple of seconds.

The answer is polled every four seconds, so switching the theme in Settings › Appearance (or
`xfconf-query -c xsettings -p /Net/ThemeName -s Adwaita-dark`) recolours WordCraft within a few
seconds without a restart. GTK's `Gtk/ApplicationPreferDarkTheme` switch wins over the theme name
when both are set (GTK draws dark Adwaita while the switch is on). The app's own Light or Dark
choice always wins over the desktop's; `system` with no answer stays light.

## Interface scaling

winit asks, in order: the XSETTINGS `Xft/DPI` of the running settings manager, the `Xft.dpi` X
resource, the physical size the X screen reports. A bare X session has no settings manager, and the
Termux:X11 screen reports 0 mm, so the scale stays 100%. A full XFCE session (`~/start-xfce.sh`)
supplies both:

- Settings › Appearance › Settings › Window Scaling (1×, 1.5×, 2×), or
- `xfconf-query -c xsettings -p /Xft/DPI -s 144` (144 dpi, 150%). XFCE 4.20 stores the DPI
  itself and multiplies by 1024 when publishing; the 4.18-era `-s 147456` (DPI × 1024) is read as
  98304 dpi there and clamped to 500, which makes the whole desktop five times too large.

`xfsettingsd` republishes the value and winit applies it live. Without a settings manager, set
`Xft.dpi` before the app starts instead: `echo 'Xft.dpi: 144' | xrdb -merge`.

Starting at a scaled DPI restores the window in points (a 922×589 window is 1383×884 physical at
150%). Changing the DPI while the app runs rescales the interface live, but eframe re-applies the
old physical window size then, so the window comes back too large: set the DPI before launching, or
resize the window afterwards.

The app's own File › Options › General › Interface size (`ui.interfaceSize`) can override the
system's scale: Auto follows the display scale and shows the size it draws at, a percentage (50–300)
is absolute, so 100% is one screen pixel per point. Document zoom is separate (`view.zoom`).

## Settings and saved files

- Preferences live in `~/.config/wordcraft/ui.json` — the interface theme, the interface size
  (`uiScale`), window position and size, and the recent-files list. They are written when the app
  exits (File › Exit, the window's close button, quitting from the task switcher), not on every
  change; the preference commands that promise it (`ui.recentRemove`, `ui.recentClear`) save at
  once.
- Saving goes through WordCraft's own file browser (`WORDCRAFT_FILE_DIALOG=builtin`, since there is
  no portal): Save and Save As write `.docx`, and opening the result gives back its text. Verified
  on this port: a 10.6 KB `.docx` saved from the sample reopens with 41 paragraphs and 364 words.
- The control channel's key file (`~/.config/wordcraft/control-key.local-<port>`) is removed when
  the app exits.

## Install as a Termux package

The build can be packaged as an ordinary Termux `.deb`:

```bash
bash build-termux.sh
bash package-termux.sh                              # -> dist/wordcraft_0.0.1_aarch64.deb
pkg install ./dist/wordcraft_0.0.1_aarch64.deb      # or: dpkg -i <file>
wordcraft
```

The package puts the launcher in `$PREFIX/bin/wordcraft` and the app in `$PREFIX/libexec/wordcraft/`,
and installs a desktop entry and icon. It declares the X11 and Mesa dependencies, and deliberately
does **not** depend on the VirGL server: rendering uses Mesa's software rasteriser (llvmpipe) by
default. On devices where VirGL works, install `virglrenderer-android`, start
`virgl_test_server_android`, and run `GALLIUM_DRIVER=virpipe wordcraft`.
`pkg uninstall wordcraft` removes it. `wordcraft-cli` is included when it has been built
(`cargo build -p wordcraft-cli`).

## Builds and releases

`.github/workflows/build.yml` builds the debug app on Linux x86-64 for every push and pull request
and attaches it to the run (Artifacts). `.github/workflows/release.yml` builds the release app and
publishes a GitHub release:

- push a tag: `git tag v0.0.1 && git push origin v0.0.1`, or
- run the workflow by hand (Actions › Release › Run workflow): it bumps the workspace version from
  the latest tag — patch by default, starting at 0.0.1 — commits the bump, builds and releases. You
  can pick minor or major, or type an exact version.

Neither workflow produces the Termux binary: that needs Termux's own `$PREFIX` libraries, which the
x86-64 runner does not have, so the release asset is the desktop (Linux x86-64) build. The tablet
build stays `build-termux.sh` on-device, or `.github/workflows/termux.yml` (manual): it builds the
aarch64 `.deb` inside the `termux/termux-docker:aarch64` container on GitHub's free arm64 runner and
can attach it to a release by tag.

## Known limits

- **No Vulkan.** The Mali driver exposes only `VK_KHR_android_surface`; wgpu cannot present a
  Vulkan surface to X11. The OpenGL backend through VirGL is the hardware path.
- **No xdg-desktop-portal**, so WordCraft uses its own file browser (`WORDCRAFT_FILE_DIALOG=builtin`).
- **No AT-SPI accessibility bus**, so screen-reader integration is inactive (the app's own
  accessibility tree still builds).
- **No self-update**: this is a local port, not a packaged release. The update check also fails
  on Android because `rustls-platform-verifier` wants an Android activity it does not get here;
  the failure is caught and logged, nothing else is affected.

Harmless messages you will see at startup:

- `xkbcommon_dl: Failed loading libxkbcommon*.so.0` — Termux ships the libraries without the
  `.so.N` soname; the loader then opens `libxkbcommon.so`, which works (a missing library would
  abort the process, this does not).
- `wgpu_hal::gles::egl: eglCreateContext ... dri2_create_context` followed by
  `GL_MAX_ELEMENT_INDEX` — wgpu first asks for a desktop OpenGL 3.3 context, which Mesa's EGL
  refuses here; it then creates the OpenGL ES 3.0 context that runs on VirGL. Rendering is on the
  GPU (the wgpu adapter reports `virgl (Mali-G615 MC2)`).
