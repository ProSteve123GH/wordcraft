#!/data/data/com.termux/files/usr/bin/bash
# Build the WordCraft arm64 debug APK on a low-RAM Android device.
#
# Prefer the GitHub Actions workflow (.github/workflows/android.yml) for full builds;
# this script is for fast local checks, installs and smoke tests. It deliberately keeps
# one Cargo job and a 1 GiB Gradle heap so Android's LMK is less likely to kill Termux.
#
# Requirements: Rust + aarch64-linux-android target, cargo-ndk, Android SDK/NDK,
# Java 21. Set ANDROID_HOME (or ANDROID_SDK_ROOT); the NDK version must match
# apps/wordcraft-android/android/app/build.gradle.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ANDROID_DIR="$ROOT/apps/wordcraft-android/android"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android-sdk}}"
NDK_VERSION="29.0.14206865"
NDK_DIR="$SDK/ndk/$NDK_VERSION"
INSTALL=0

for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        *) echo "usage: $0 [--install]" >&2; exit 2 ;;
    esac
done

if [[ ! -d "$SDK" ]]; then
    echo "Android SDK not found at $SDK; set ANDROID_HOME or run the GitHub Actions build." >&2
    exit 1
fi
if [[ ! -d "$NDK_DIR" ]]; then
    echo "NDK $NDK_VERSION not found at $NDK_DIR; install it with sdkmanager." >&2
    exit 1
fi
if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "cargo-ndk is missing; install with: cargo install cargo-ndk --locked" >&2
    exit 1
fi

export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"
export ANDROID_NDK_HOME="$NDK_DIR"
export CARGO_BUILD_JOBS=1
export CARGO_INCREMENTAL=0
# Keep Gradle from starting a daemon or extra workers.
export GRADLE_OPTS="${GRADLE_OPTS:-} -Dorg.gradle.daemon=false -Dorg.gradle.workers.max=1"

avail_kb=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)
if [[ "${avail_kb:-0}" -lt 1200000 ]]; then
    echo "warning: only ${avail_kb:-0} kB available; close other apps before a native build." >&2
fi

echo "==> building libwordcraft_android.so (arm64, low-RAM profile)"
cargo ndk \
    --platform 26 \
    -t arm64-v8a \
    -o "$ANDROID_DIR/app/src/main/jniLibs" \
    build --profile android -p wordcraft-android

echo "==> assembling debug APK"
(cd "$ANDROID_DIR" && ./gradlew --no-daemon assembleDebug)

APK="$ANDROID_DIR/app/build/outputs/apk/debug/app-debug.apk"
ls -lh "$APK"

if [[ "$INSTALL" -eq 1 ]]; then
    if ! command -v adb >/dev/null 2>&1; then
        echo "adb is missing; install android-tools or use the mobile UI." >&2
        exit 1
    fi
    echo "==> installing $APK"
    adb install -r "$APK"
    adb shell am force-stop com.storytold.wordcraft
    adb shell monkey -p com.storytold.wordcraft -c android.intent.category.LAUNCHER 1 >/dev/null
fi

echo "APK: $APK"
