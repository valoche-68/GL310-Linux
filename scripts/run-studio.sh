#!/usr/bin/env bash
# Wayland + NVIDIA safe launcher for GL310 Studio (Tauri v2 / WebKitGTK 4.1).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Modern WebKitGTK (2.46+) + NVIDIA (555+) support hardware DMABUF rendering on Wayland natively.
# Only disable DMABUF when explicitly requested (GL310_SOFTWARE_RENDER=1) or on legacy NVIDIA (<555),
# because forcing software rasterization on modern WebKitGTK spikes CPU usage.
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

# Locate binary: installed PATH -> local release build -> local debug build
if command -v gl310-studio >/dev/null 2>&1; then
    BIN="$(command -v gl310-studio)"
elif [[ -x "$REPO_ROOT/target/release/gl310-studio" ]]; then
    BIN="$REPO_ROOT/target/release/gl310-studio"
elif [[ -x "$REPO_ROOT/target/debug/gl310-studio" ]]; then
    BIN="$REPO_ROOT/target/debug/gl310-studio"
else
    echo "ERROR: gl310-studio binary not found. Build with: cargo build --release -p gl310-studio" >&2
    exit 1
fi

exec "$BIN" "$@"
