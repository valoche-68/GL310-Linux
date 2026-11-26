#!/usr/bin/env bash
# Builds a clean local Debian package (.deb) containing:
#   - /usr/bin/gl310-cli
#   - /usr/bin/gl310-studio
#   - /usr/bin/gl310-studio-launch (Wayland + NVIDIA safe wrapper)
#   - /etc/udev/rules.d/70-avermedia-gl310.rules
#   - /usr/lib/systemd/system/gl310-kvm.service
#   - /usr/share/applications/gl310-studio.desktop
#   - /usr/share/icons/hicolor/scalable/apps/gl310-studio.svg & 128x128 PNG
#
# Never includes proprietary AVerMedia firmware binaries.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
PKG_NAME="gl310-linux"
DIST_DIR="$REPO_ROOT/dist"
STAGE_DIR="$DIST_DIR/${PKG_NAME}_${VERSION}_${ARCH}"
DEB_PATH="$DIST_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"

CLI_BIN="$REPO_ROOT/target/release/gl310-cli"
STUDIO_BIN="$REPO_ROOT/target/release/gl310-studio"

if [[ ! -x "$CLI_BIN" || ! -x "$STUDIO_BIN" ]]; then
    echo "[package_deb] Release binaries missing — building workspace..."
    cargo build --release -p gl310-cli -p gl310-studio
fi

rm -rf "$STAGE_DIR"
mkdir -p \
    "$STAGE_DIR/DEBIAN" \
    "$STAGE_DIR/usr/bin" \
    "$STAGE_DIR/etc/udev/rules.d" \
    "$STAGE_DIR/usr/lib/systemd/system" \
    "$STAGE_DIR/usr/share/applications" \
    "$STAGE_DIR/usr/share/icons/hicolor/scalable/apps" \
    "$STAGE_DIR/usr/share/icons/hicolor/128x128/apps" \
    "$STAGE_DIR/usr/share/doc/$PKG_NAME"

install -m 0755 "$CLI_BIN" "$STAGE_DIR/usr/bin/gl310-cli"
install -m 0755 "$STUDIO_BIN" "$STAGE_DIR/usr/bin/gl310-studio"

cat > "$STAGE_DIR/usr/bin/gl310-studio-launch" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
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
exec /usr/bin/gl310-studio "$@"
EOF
chmod 0755 "$STAGE_DIR/usr/bin/gl310-studio-launch"

install -m 0644 "$REPO_ROOT/udev/70-avermedia-gl310.rules" "$STAGE_DIR/etc/udev/rules.d/70-avermedia-gl310.rules"
sed 's|/usr/local/bin/gl310-cli|/usr/bin/gl310-cli|g' \
    "$REPO_ROOT/systemd/gl310-kvm.service" > "$STAGE_DIR/usr/lib/systemd/system/gl310-kvm.service"
chmod 0644 "$STAGE_DIR/usr/lib/systemd/system/gl310-kvm.service"

install -m 0644 "$REPO_ROOT/apps/gl310-studio/ui/assets/icon.svg" \
    "$STAGE_DIR/usr/share/icons/hicolor/scalable/apps/gl310-studio.svg"
install -m 0644 "$REPO_ROOT/apps/gl310-studio/src-tauri/icons/128x128.png" \
    "$STAGE_DIR/usr/share/icons/hicolor/128x128/apps/gl310-studio.png"
install -m 0644 "$REPO_ROOT/LICENSE" "$STAGE_DIR/usr/share/doc/$PKG_NAME/copyright"

cat > "$STAGE_DIR/usr/share/applications/gl310-studio.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=GL310 Studio
GenericName=AVerMedia LGP Lite Capture Studio
Comment=Userspace capture studio, OBS bridge & KVM monitor for AVerMedia GL310 (07ca:c835)
Exec=/usr/bin/gl310-studio-launch
Icon=gl310-studio
Terminal=false
Categories=AudioVideo;Video;Recorder;
Keywords=AVerMedia;GL310;LGP;HDMI;OBS;Capture;KVM;
StartupWMClass=gl310-studio
EOF
chmod 0644 "$STAGE_DIR/usr/share/applications/gl310-studio.desktop"

INSTALLED_KB="$(du -sk "$STAGE_DIR" | awk '{print $1}')"

cat > "$STAGE_DIR/DEBIAN/control" <<EOF
Package: $PKG_NAME
Version: $VERSION
Section: video
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_KB
Depends: libusb-1.0-0, libwebkit2gtk-4.1-0, libgtk-3-0, ffmpeg
Recommends: v4l2loopback-utils, pipewire-bin, pulseaudio-utils, libayatana-appindicator3-1
Maintainer: valoche-68 <valoche-68@users.noreply.github.com>
Homepage: https://github.com/valoche-68/GL310-Linux
Description: Userspace Linux driver, CLI, KVM daemon & Tauri Studio for AVerMedia GL310
 Complete 100% userspace Linux solution for the AVerMedia Live Gamer Portable
 Lite (GL310, USB 07ca:c835). Includes gl310-cli (OBS bridge, capture, record,
 V4L2, Raspberry Pi MJPEG KVM daemon) and GL310 Studio (Tauri v2 desktop app).
 Proprietary AVerMedia firmwares are not included; run 'gl310-cli firmware fetch'
 or 'gl310-cli firmware import <path>' after installation.
EOF

cat > "$STAGE_DIR/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=usb --attr-match=idVendor=07ca || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
echo "GL310-Linux installed. Run 'gl310-cli firmware fetch' (or 'gl310-cli firmware import <zip>') to install the verified AVerMedia firmwares."
EOF
chmod 0755 "$STAGE_DIR/DEBIAN/postinst"

mkdir -p "$DIST_DIR"
dpkg-deb --build --root-owner-group "$STAGE_DIR" "$DEB_PATH"
rm -rf "$STAGE_DIR"

echo "[package_deb] Built: $DEB_PATH"
dpkg-deb --info "$DEB_PATH"
dpkg-deb --contents "$DEB_PATH"
