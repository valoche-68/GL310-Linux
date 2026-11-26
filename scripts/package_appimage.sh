#!/usr/bin/env bash
# Builds a clean, standalone multi-distro AppImage locally for GL310 Studio + gl310-cli:
#   dist/GL310-Studio-<version>-x86_64.AppImage
#
# Never includes proprietary AVerMedia firmware binaries.
# Downloads the official AppImage/appimagetool binary into ~/.cache/gl310-linux/tools/
# (strictly outside the Git repository) with SHA-256 verification.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
ARCH="$(uname -m)"
DIST_DIR="$REPO_ROOT/dist"
APPDIR="$DIST_DIR/GL310-Studio.AppDir"
OUT_APPIMAGE="$DIST_DIR/GL310-Studio-${VERSION}-${ARCH}.AppImage"

CLI_BIN="$REPO_ROOT/target/release/gl310-cli"
STUDIO_BIN="$REPO_ROOT/target/release/gl310-studio"

if [[ ! -x "$CLI_BIN" || ! -x "$STUDIO_BIN" ]]; then
    echo "[package_appimage] Release binaries missing — building workspace..."
    cargo build --release -p gl310-cli -p gl310-studio
fi

# Locate or fetch official AppImage/appimagetool (never inside the Git repo)
TOOL_CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/gl310-linux/tools"
APPIMAGETOOL=""
if command -v appimagetool >/dev/null 2>&1; then
    APPIMAGETOOL="$(command -v appimagetool)"
else
    mkdir -p "$TOOL_CACHE"
    CACHED_TOOL="$TOOL_CACHE/appimagetool-${ARCH}.AppImage"
    EXPECTED_SHA256_X86_64="e8c1169c4af66c3184d7ee110713d4be04befe671fa358e2c1591ee840553b03"
    if [[ ! -x "$CACHED_TOOL" ]]; then
        TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"
        echo "[package_appimage] Downloading official appimagetool from $TOOL_URL..."
        curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
            -o "${CACHED_TOOL}.part" "$TOOL_URL"
        if [[ "$ARCH" == "x86_64" ]]; then
            ACTUAL_SHA="$(sha256sum "${CACHED_TOOL}.part" | awk '{print $1}')"
            if [[ "$ACTUAL_SHA" != "$EXPECTED_SHA256_X86_64" ]]; then
                echo "[package_appimage] WARNING: appimagetool continuous SHA-256 changed ($ACTUAL_SHA)." >&2
                echo "                   Verify the latest digest on https://github.com/AppImage/appimagetool/releases/tag/continuous" >&2
                echo "                   or set GL310_ALLOW_NEW_APPIMAGETOOL=1 to proceed." >&2
                if [[ "${GL310_ALLOW_NEW_APPIMAGETOOL:-0}" != "1" ]]; then
                    rm -f "${CACHED_TOOL}.part"
                    exit 1
                fi
            else
                echo "[package_appimage] appimagetool SHA-256 verified: $ACTUAL_SHA"
            fi
        fi
        chmod 0755 "${CACHED_TOOL}.part"
        mv "${CACHED_TOOL}.part" "$CACHED_TOOL"
    fi
    APPIMAGETOOL="$CACHED_TOOL"
fi

rm -rf "$APPDIR" "$OUT_APPIMAGE"
mkdir -p \
    "$APPDIR/usr/bin" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/128x128/apps" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
    "$APPDIR/usr/share/gl310-linux"

install -m 0755 "$STUDIO_BIN" "$APPDIR/usr/bin/gl310-studio"
install -m 0755 "$CLI_BIN" "$APPDIR/usr/bin/gl310-cli"
install -m 0644 "$REPO_ROOT/udev/70-avermedia-gl310.rules" "$APPDIR/usr/share/gl310-linux/70-avermedia-gl310.rules"
install -m 0644 "$REPO_ROOT/systemd/gl310-kvm.service" "$APPDIR/usr/share/gl310-linux/gl310-kvm.service"
install -m 0644 "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/gl310-linux/LICENSE"

install -m 0644 "$REPO_ROOT/apps/gl310-studio/src-tauri/icons/128x128.png" \
    "$APPDIR/usr/share/icons/hicolor/128x128/apps/gl310-studio.png"
install -m 0644 "$REPO_ROOT/apps/gl310-studio/ui/assets/icon.svg" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps/gl310-studio.svg"
cp "$REPO_ROOT/apps/gl310-studio/src-tauri/icons/128x128.png" "$APPDIR/gl310-studio.png"
ln -sf gl310-studio.png "$APPDIR/.DirIcon"

cat > "$APPDIR/gl310-studio.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=GL310 Studio
GenericName=AVerMedia LGP Lite Capture Studio
Comment=Userspace capture studio, OBS bridge & KVM monitor for AVerMedia GL310 (07ca:c835)
Exec=gl310-studio
Icon=gl310-studio
Terminal=false
Categories=AudioVideo;Video;Recorder;
Keywords=AVerMedia;GL310;LGP;HDMI;OBS;Capture;KVM;
StartupWMClass=gl310-studio
EOF
cp "$APPDIR/gl310-studio.desktop" "$APPDIR/usr/share/applications/gl310-studio.desktop"

cat > "$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$HERE/usr/bin:$PATH"

# Allow invoking the bundled CLI directly: ./GL310-Studio-*.AppImage --cli doctor
if [[ "${1:-}" == "--cli" ]]; then
    shift
    exec "$HERE/usr/bin/gl310-cli" "$@"
fi

# Modern WebKitGTK (2.46+) + NVIDIA (555+) support hardware DMABUF rendering natively.
# Only disable DMABUF when GL310_SOFTWARE_RENDER=1 or on legacy NVIDIA (<555) under Wayland.
if [[ "${GL310_SOFTWARE_RENDER:-0}" == "1" ]]; then
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
elif [[ -z "${WEBKIT_DISABLE_DMABUF_RENDERER+x}" ]] && [[ "${XDG_SESSION_TYPE:-}" == "wayland" || -n "${WAYLAND_DISPLAY:-}" ]]; then
    if [[ -r /proc/driver/nvidia/version ]]; then
        NV_MAJOR="$(sed -nE 's/.*Kernel Module.*[[:space:]]([0-9]+)\.[0-9]+.*/\1/p' /proc/driver/nvidia/version | head -n1 || true)"
        if [[ -n "$NV_MAJOR" && "$NV_MAJOR" -lt 555 ]]; then
            export WEBKIT_DISABLE_DMABUF_RENDERER=1
        fi
    fi
fi

exec "$HERE/usr/bin/gl310-studio" "$@"
EOF
chmod 0755 "$APPDIR/AppRun"

# Anti-leak guard: verify zero proprietary AVerMedia artifacts inside AppDir
if find "$APPDIR" -type f | grep -iE '(qpvidfwusb|qpaudfwusb|aver330usb|\.(bin|sys|cat|inf|exe|dll|ax|zip)$)' >/dev/null 2>&1; then
    echo "[package_appimage] ERROR: Forbidden proprietary artifact detected in $APPDIR!" >&2
    rm -rf "$APPDIR"
    exit 1
fi

mkdir -p "$DIST_DIR"
ARCH="$ARCH" "$APPIMAGETOOL" --appimage-extract-and-run --no-appstream "$APPDIR" "$OUT_APPIMAGE"
chmod 0755 "$OUT_APPIMAGE"
rm -rf "$APPDIR"

echo "[package_appimage] Built: $OUT_APPIMAGE ($(du -h "$OUT_APPIMAGE" | awk '{print $1}'))"
