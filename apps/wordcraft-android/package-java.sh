#!/usr/bin/env bash
# Package the Java MainActivity into a cargo-apk APK and sign it again.
#
# `cargo apk` only builds Rust. MainActivity is the Back/IME hook, device listener and window
# inset source, so an APK built by cargo-apk alone names an Activity that is not in its
# classes.dex and will fail to launch. The CI workflow runs this script after `cargo apk build`;
# use it the same way for a local build.
#
# Usage:
#   ANDROID_HOME=/path/to/sdk \
#   CARGO_APK_RELEASE_KEYSTORE=/path/to/test.jks \
#   CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android \
#   apps/wordcraft-android/package-java.sh target/aarch64-linux-android/release/apk/wordcraft.apk
#
# Requires build-tools 34.0.0, platform android-34, a JDK (javac), zip and unzip.
set -euo pipefail

APK="${1:?usage: package-java.sh path/to/wordcraft.apk}"
: "${ANDROID_HOME:?set ANDROID_HOME to an Android SDK with build-tools 34.0.0 and platform android-34}"
: "${CARGO_APK_RELEASE_KEYSTORE:?set CARGO_APK_RELEASE_KEYSTORE to the signing keystore}"
: "${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:?set CARGO_APK_RELEASE_KEYSTORE_PASSWORD}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
JAVA_SRC="$SCRIPT_DIR/java/ai/storyteller/wordcraft/MainActivity.java"
BUILD_TOOLS="$ANDROID_HOME/build-tools/34.0.0"
PLATFORM="$ANDROID_HOME/platforms/android-34/android.jar"
CLASSES="$(mktemp -d)"
DEX="$(mktemp -d)"
cleanup() {
    rm -rf "$CLASSES" "$DEX"
}
trap cleanup EXIT

if [ ! -f "$APK" ]; then
    echo "APK not found: $APK" >&2
    exit 1
fi
if [ ! -f "$PLATFORM" ]; then
    echo "android.jar not found: $PLATFORM" >&2
    exit 1
fi
if [ ! -x "$BUILD_TOOLS/d8" ] || [ ! -x "$BUILD_TOOLS/zipalign" ] || [ ! -x "$BUILD_TOOLS/apksigner" ]; then
    echo "build-tools 34.0.0 (d8, zipalign, apksigner) not found under $BUILD_TOOLS" >&2
    exit 1
fi

APK_ABS="$(realpath "$APK")"

# Compile against the Android API, not the host JDK's bootclasspath. Java 8 keeps the source
# compatible with every Android API level the app supports; d8 desugars the lambda.
javac -source 8 -target 8 -Xlint:-options \
    -classpath "$PLATFORM" \
    -d "$CLASSES" \
    "$JAVA_SRC"

# Dex the compiled classes. MainActivity is the only class (the InputDeviceListener methods are
# on the Activity itself and the inset lambda is a synthetic method), but find keeps this correct
# if more Java is added later.
"$BUILD_TOOLS/d8" --release --min-api 26 --lib "$PLATFORM" \
    --output "$DEX" \
    $(find "$CLASSES" -name '*.class' -print)

# classes.dex goes at the APK root. Adding it changes the archive, so remove cargo-apk's v1
# signature files, align, and sign with the same keystore again.
(cd "$DEX" && zip -q "$APK_ABS" classes.dex)
zip -q -d "$APK_ABS" 'META-INF/*.RSA' 'META-INF/*.SF' 'META-INF/*.MF' || true
"$BUILD_TOOLS/zipalign" -f -p 4 "$APK_ABS" "$APK_ABS.aligned"
/bin/mv -f "$APK_ABS.aligned" "$APK_ABS"
"$BUILD_TOOLS/apksigner" sign \
    --ks "$CARGO_APK_RELEASE_KEYSTORE" \
    --ks-key-alias wordcraft \
    --ks-pass "pass:$CARGO_APK_RELEASE_KEYSTORE_PASSWORD" \
    --key-pass "pass:$CARGO_APK_RELEASE_KEYSTORE_PASSWORD" \
    --out "$APK_ABS.signed" "$APK_ABS"
/bin/mv -f "$APK_ABS.signed" "$APK_ABS"

unzip -l "$APK_ABS" | grep -q 'classes.dex'
echo "packaged MainActivity.dex into $APK_ABS and signed it"
