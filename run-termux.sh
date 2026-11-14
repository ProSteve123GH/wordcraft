#!/data/data/com.termux/files/usr/bin/bash
#
# Run WordCraft natively in Termux (no proot), on Termux:X11, with hardware acceleration.
#
#   bash ~/wordcraft/run-termux.sh            # normal start
#   bash ~/wordcraft/run-termux.sh --sample   # start with the sample document
#
# Graphics path: wgpu's OpenGL backend -> Mesa EGL -> VirGL -> the device's Mali GPU.
# (`virgl_test_server_android` hands Mesa's `virpipe` driver the phone's real GLES driver, so
# rendering happens on the GPU, not in llvmpipe.) Vulkan is deliberately not used: this Mali
# driver only exposes VK_KHR_android_surface, so wgpu cannot present a Vulkan surface to X11.
#
# Needs the Termux:X11 app open on screen for DISPLAY=:0.

set -u

export DISPLAY=:0

# --- Mesa / VirGL (hardware) ---
export GALLIUM_DRIVER=virpipe
export MESA_GL_VERSION_OVERRIDE=4.0
export MESA_GLSL_VERSION_OVERRIDE=400
export vblank_mode=0
unset LIBGL_ALWAYS_SOFTWARE

# --- wgpu / winit: OpenGL over X11 ---
export WGPU_BACKEND=gl
export WINIT_UNIX_BACKEND=x11
export WORDCRAFT_WAYLAND=0
# Android has no writable /tmp; Termux:X11 keeps its socket in $PREFIX/tmp/.X11-unix.
export X11_UNIX_SOCKET_DIR="$PREFIX/tmp/.X11-unix"

# No xdg-desktop-portal in Termux: use WordCraft's own file browser.
export WORDCRAFT_FILE_DIALOG=builtin

export RUST_LOG="${RUST_LOG:-info}"

LOGDIR="$PREFIX/tmp"
mkdir -p "$LOGDIR"
BIN="$HOME/wordcraft/target/debug/wordcraft"

if [ ! -x "$BIN" ]; then
    echo "wordcraft: $BIN is missing — build it first:  bash ~/wordcraft/build-termux.sh" >&2
    exit 1
fi

# 1. VirGL server: exposes the phone's real GPU to Mesa inside Termux.
# Both servers detach (setsid) so closing this terminal, or a caller that reaps its process
# group, does not take the display down with it.
if ! pgrep -x virgl_test_server_android >/dev/null 2>&1; then
    echo "[*] starting the VirGL server"
    setsid nohup virgl_test_server_android >"$LOGDIR/virgl-server.log" 2>&1 </dev/null &
    sleep 2
fi

# 2. The X server inside Termux. Check the process, not the socket: a crashed server leaves
#    $LOGDIR/.X11-unix/X0 behind, and connecting to that stale socket fails with XOpenDisplayFailed.
if ! pgrep -f "^termux-x11 .*:0" >/dev/null 2>&1; then
    echo "[*] starting Termux:X11 on :0 (open the Termux:X11 app to see the window)"
    rm -f "$LOGDIR/.X0-lock" "$LOGDIR/.X11-unix/X0"
    setsid nohup termux-x11 :0 -ac >"$LOGDIR/termux-x11.log" 2>&1 </dev/null &
    for _ in $(seq 1 20); do
        [ -S "$LOGDIR/.X11-unix/X0" ] && break
        sleep 0.5
    done
fi

# 3. A window manager, so the window can be moved and focused (XFCE's if it is already running).
if ! pgrep -x xfwm4 >/dev/null 2>&1 && command -v xfwm4 >/dev/null 2>&1; then
    xfwm4 --daemon >/dev/null 2>&1 || true
fi

echo "[*] starting WordCraft (GPU: $(glxinfo -B 2>/dev/null | sed -n 's/^ *OpenGL renderer string: *//p' | head -1))"
exec "$BIN" "$@"
