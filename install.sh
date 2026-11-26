#!/usr/bin/env bash
# Universal multi-distro installer for GL310-Linux:
#   ./install.sh --studio    : Build & install gl310-cli + gl310-studio + desktop launcher + udev
#   ./install.sh --cli-only  : Build & install gl310-cli + udev (zero GUI deps, works on headless/Pi)
#   ./install.sh --kvm       : Build & install gl310-cli + systemd gl310-kvm.service + udev
#   ./install.sh --uninstall : Remove installed user binaries and desktop launcher
set -euo pipefail

ORIG_PWD="$PWD"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_ROOT"

MODE="studio"
SKIP_UDEV=0
KVM_FW_READY=0

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Options:
  --studio      Install gl310-cli, gl310-studio (Tauri v2), desktop icon, and udev rule (default)
  --cli-only    Install gl310-cli and udev rule only (zero GUI dependencies)
  --kvm         Install gl310-cli, systemd gl310-kvm.service (Raspberry Pi / headless), and udev rule
  --no-udev     Skip installing /etc/udev/rules.d/70-avermedia-gl310.rules (no sudo needed)
  --uninstall   Uninstall user-installed binaries and desktop files
  -h, --help    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --studio) MODE="studio"; shift ;;
        --cli-only) MODE="cli-only"; shift ;;
        --kvm) MODE="kvm"; shift ;;
        --no-udev) SKIP_UDEV=1; shift ;;
        --uninstall) MODE="uninstall"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

USER_BIN="$HOME/.local/bin"
USER_APPS="$HOME/.local/share/applications"
USER_ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"

VID_FW_SHA256="8da30f6dc2d4fee5aeb3e2e06df5581db4c01d146c50c21f6a5d4824025a47d1"
AUD_FW_SHA256="0e8ea66b6a3ddb68f9a283f10155df90ce8d6f46599ed0ecf6f308d6c90210f4"

verify_firmware_dir() {
    local dir="$1"
    [[ -n "$dir" ]] || return 1
    local vid="$dir/qpvidfwusb.bin"
    local aud="$dir/qpaudfwusb.bin"
    [[ -f "$vid" && -r "$vid" && -f "$aud" && -r "$aud" ]] || return 1
    local vid_sha aud_sha
    vid_sha="$(sha256sum "$vid" | awk '{print $1}')"
    aud_sha="$(sha256sum "$aud" | awk '{print $1}')"
    [[ "$vid_sha" == "$VID_FW_SHA256" && "$aud_sha" == "$AUD_FW_SHA256" ]]
}

find_verified_local_firmware() {
    local sudo_home=""
    if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]]; then
        sudo_home="$(getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6 || true)"
    fi
    local candidates=(
        "$REPO_ROOT/firmware"
        "$ORIG_PWD/firmware"
        "${GL310_FIRMWARE:-}"
        "${XDG_DATA_HOME:-$HOME/.local/share}/gl310-linux/firmware"
        "$HOME/.local/share/gl310-linux/firmware"
    )
    if [[ -n "$sudo_home" ]]; then
        candidates+=("$sudo_home/.local/share/gl310-linux/firmware")
    fi
    local cand
    for cand in "${candidates[@]}"; do
        [[ -n "$cand" ]] || continue
        if verify_firmware_dir "$cand"; then
            printf '%s\n' "$cand"
            return 0
        fi
    done
    return 1
}

if [[ "$MODE" == "uninstall" ]]; then
    rm -f "$USER_BIN/gl310-cli" "$USER_BIN/gl310-studio" "$USER_BIN/gl310-studio-launch"
    rm -f "$USER_APPS/gl310-studio.desktop" "$USER_ICONS/gl310-studio.svg"
    echo "[install.sh] Removed user-installed GL310-Linux files."
    echo "             Note: system udev/systemd files (if installed) can be removed with:"
    echo "             sudo rm -f /etc/udev/rules.d/70-avermedia-gl310.rules /etc/systemd/system/gl310-kvm.service"
    exit 0
fi

case "$MODE" in
    studio)
        mkdir -p "$USER_BIN"
        echo "[1/3] Building gl310-cli and gl310-studio (release)..."
        cargo build --release -p gl310-cli -p gl310-studio
        install -m 0755 target/release/gl310-cli "$USER_BIN/gl310-cli"
        install -m 0755 target/release/gl310-studio "$USER_BIN/gl310-studio"
        cat > "$USER_BIN/gl310-studio-launch" <<EOF
#!/usr/bin/env bash
set -euo pipefail
if [[ "\${GL310_SOFTWARE_RENDER:-0}" == "1" ]]; then
    export WEBKIT_DISABLE_DMABUF_RENDERER=1
elif [[ -z "\${WEBKIT_DISABLE_DMABUF_RENDERER+x}" ]] && [[ "\${XDG_SESSION_TYPE:-}" == "wayland" || -n "\${WAYLAND_DISPLAY:-}" ]]; then
    if [[ -r /proc/driver/nvidia/version ]]; then
        NV_MAJOR="\$(sed -nE 's/.*Kernel Module.*[[:space:]]([0-9]+)\.[0-9]+.*/\1/p' /proc/driver/nvidia/version | head -n1 || true)"
        if [[ -n "\$NV_MAJOR" && "\$NV_MAJOR" -lt 555 ]]; then
            export WEBKIT_DISABLE_DMABUF_RENDERER=1
        fi
    fi
fi
exec "$USER_BIN/gl310-studio" "\$@"
EOF
        chmod 0755 "$USER_BIN/gl310-studio-launch"

        mkdir -p "$USER_APPS" "$USER_ICONS"
        install -m 0644 apps/gl310-studio/ui/assets/icon.svg "$USER_ICONS/gl310-studio.svg"
        cat > "$USER_APPS/gl310-studio.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=GL310 Studio
GenericName=AVerMedia LGP Lite Capture Studio
Comment=Userspace capture studio, OBS bridge & KVM monitor for AVerMedia GL310
Exec=$USER_BIN/gl310-studio-launch
Icon=gl310-studio
Terminal=false
Categories=AudioVideo;Video;Recorder;
Keywords=AVerMedia;GL310;LGP;HDMI;OBS;Capture;KVM;
StartupWMClass=gl310-studio
EOF
        command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$USER_APPS" || true
        echo "[2/3] Installed CLI, Studio binary, and desktop launcher into $USER_BIN."
        ;;
    cli-only)
        mkdir -p "$USER_BIN"
        echo "[1/3] Building gl310-cli (release, zero GUI dependencies)..."
        cargo build --release -p gl310-cli
        install -m 0755 target/release/gl310-cli "$USER_BIN/gl310-cli"
        echo "[2/3] Installed $USER_BIN/gl310-cli."
        ;;
    kvm)
        echo "[1/3] Building gl310-cli (release, headless KVM mode)..."
        cargo build --release -p gl310-cli
        echo "[2/3] Installing system binary /usr/local/bin/gl310-cli and systemd gl310-kvm.service..."
        sudo install -m 0755 target/release/gl310-cli /usr/local/bin/gl310-cli
        sudo install -m 0644 systemd/gl310-kvm.service /etc/systemd/system/gl310-kvm.service
        sudo systemctl daemon-reload

        FW_SRC="$(find_verified_local_firmware || true)"
        if [[ -n "$FW_SRC" ]]; then
            echo "      Installing verified AVerMedia firmwares from $FW_SRC -> /lib/firmware/avermedia/..."
            sudo install -d -m 0755 /lib/firmware/avermedia
            sudo install -m 0644 "$FW_SRC/qpvidfwusb.bin" /lib/firmware/avermedia/qpvidfwusb.bin
            sudo install -m 0644 "$FW_SRC/qpaudfwusb.bin" /lib/firmware/avermedia/qpaudfwusb.bin
            KVM_FW_READY=1
        elif verify_firmware_dir "/lib/firmware/avermedia"; then
            echo "      Verified AVerMedia firmwares already installed in /lib/firmware/avermedia/."
            KVM_FW_READY=1
        else
            echo "      NOTE: Verified AVerMedia firmwares were not found in ./firmware/ or ~/.local/share/gl310-linux/firmware/."
            echo "            Before starting gl310-kvm.service, fetch/import them and copy into /lib/firmware/avermedia/:"
            echo "              gl310-cli firmware fetch"
            echo "              sudo install -d -m 0755 /lib/firmware/avermedia"
            echo "              sudo install -m 0644 ~/.local/share/gl310-linux/firmware/*.bin /lib/firmware/avermedia/"
        fi
        echo "      Enable on boot with: sudo systemctl enable --now gl310-kvm.service"
        ;;
esac

if (( SKIP_UDEV == 0 )); then
    echo "[3/3] Installing udev rule for non-root USB 07ca:c835 access..."
    sudo bash "$REPO_ROOT/scripts/install_udev.sh"
else
    echo "[3/3] Skipped udev installation (--no-udev). Run later: sudo bash scripts/install_udev.sh"
fi

echo ""
echo "[OK] Installation complete. Next steps:"
if [[ "$MODE" == "kvm" ]]; then
    if (( KVM_FW_READY == 1 )); then
        echo "  1. Enable & start KVM daemon: sudo systemctl enable --now gl310-kvm.service"
        echo "  2. Verify environment       : gl310-cli doctor"
    else
        echo "  1. Fetch & install firmwares: gl310-cli firmware fetch && sudo install -d -m 0755 /lib/firmware/avermedia && sudo install -m 0644 ~/.local/share/gl310-linux/firmware/*.bin /lib/firmware/avermedia/"
        echo "  2. Enable & start KVM daemon: sudo systemctl enable --now gl310-kvm.service"
        echo "  3. Verify environment       : gl310-cli doctor"
    fi
else
    echo "  1. Fetch or import firmwares: gl310-cli firmware fetch"
    echo "  2. Verify environment       : gl310-cli doctor"
fi
