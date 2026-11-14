#!/data/data/com.termux/files/usr/bin/bash
#
# Package the Termux build as an installable .deb:
#
#   bash build-termux.sh              # builds target/debug/wordcraft
#   bash package-termux.sh            # -> dist/wordcraft_<version>_<arch>.deb
#
#   pkg install ./dist/wordcraft_0.0.1_aarch64.deb     # or: dpkg -i <file>
#   wordcraft
#
# Pass `release` to package a release build (`bash build-termux.sh --release` first).
# `wordcraft-cli` is included when it has been built too (`cargo build -p wordcraft-cli`).
#
# Why a wrapper instead of the binary in $PREFIX/bin: the app needs a few environment defaults
# on Termux (display, wgpu backend, file dialog, the X11 socket directory), and it lives in
# $PREFIX/libexec so a future package can replace it without touching the launcher.
set -euo pipefail
cd "$(dirname "$0")"

PROFILE="${1:-debug}"
case "$PROFILE" in
    debug)   BIN=target/debug/wordcraft;   CLI=target/debug/wordcraft-cli ;;
    release) BIN=target/release/wordcraft; CLI=target/release/wordcraft-cli ;;
    *) echo "usage: bash package-termux.sh [debug|release]" >&2; exit 2 ;;
esac

if [ ! -x "$BIN" ]; then
    echo "wordcraft: $BIN is missing — build it first: bash build-termux.sh" >&2
    [ "$PROFILE" = release ] && echo "  (for a release build: bash build-termux.sh --release)" >&2
    exit 1
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
ARCH="$(dpkg --print-architecture)"
OUT="dist/wordcraft_${VERSION}_${ARCH}.deb"
STAGE="$(mktemp -d "$PREFIX/tmp/wordcraft-deb.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT

ROOT="$STAGE/data/data/com.termux/files/usr"
install -Dm755 "$BIN" "$ROOT/libexec/wordcraft/wordcraft"
install -Dm755 packaging/termux/wordcraft.launcher "$ROOT/bin/wordcraft"
if [ -x "$CLI" ]; then
    install -Dm755 "$CLI" "$ROOT/bin/wordcraft-cli"
else
    echo "[*] wordcraft-cli not built; packaging the app only" >&2
fi
install -Dm644 packaging/termux/wordcraft.desktop "$ROOT/share/applications/wordcraft.desktop"
install -Dm644 assets/app-icon/hicolor/256x256/apps/ai.storyteller.wordcraft.png \
    "$ROOT/share/icons/hicolor/256x256/apps/ai.storyteller.wordcraft.png"
install -Dm644 packaging/termux/copyright "$ROOT/share/doc/wordcraft/copyright"

INSTALLED_KB="$(du -sk "$STAGE" | cut -f1)"
mkdir -p "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: wordcraft
Version: $VERSION
Architecture: $ARCH
Maintainer: ProSteve123GH <182427483+ProSteve123GH@users.noreply.github.com>
Installed-Size: $INSTALLED_KB
Depends: libx11, libxkbcommon, libxcursor, libxrandr, libxi, mesa, fontconfig, freetype
Section: editors
Priority: optional
Homepage: https://github.com/ProSteve123GH/wordcraft
Description: Native word processor with Word-style editing
 WordCraft is a Rust word processor (DOCX, ODT, RTF, Markdown, HTML, PDF) that draws its own
 interface on X11. This is the Termux (Android aarch64) build of the personal port.
 .
 Rendering uses Mesa's software rasteriser by default; set GALLIUM_DRIVER=virpipe with a
 running virgl_test_server_android for hardware acceleration where VirGL works.
CONTROL

mkdir -p dist
rm -f "$OUT"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT" >/dev/null
echo "[*] $OUT ($(du -h "$OUT" | cut -f1))"
