#!/usr/bin/env bash
# Non-sudo helper to create, inspect, or delete the dedicated /dev/video14 ("AVerMedia GL310")
# virtual camera node via v4l2loopback-ctl without touching existing cameras (/dev/video2, 10, 12...).
set -euo pipefail

DEV="${GL310_V4L2_DEVICE:-/dev/video14}"
LABEL="${GL310_V4L2_LABEL:-AVerMedia GL310}"

usage() {
    cat <<EOF
Usage: $(basename "$0") <add|delete|status> [device]

Manages the dedicated GL310 v4l2loopback node (default: $DEV) without sudo.
EOF
}

CMD="${1:-status}"
if [[ $# -ge 2 ]]; then
    DEV="$2"
fi

if ! command -v v4l2loopback-ctl >/dev/null 2>&1; then
    echo "ERROR: v4l2loopback-ctl not found (install v4l2loopback-utils)." >&2
    exit 1
fi

case "$CMD" in
    add|create)
        if [[ -e "$DEV" ]]; then
            echo "[v4l2_gl310] Device $DEV already exists."
        else
            v4l2loopback-ctl add -n "$LABEL" -x 1 "$DEV"
            echo "[v4l2_gl310] Created $DEV ($LABEL)."
        fi
        ;;
    delete|remove|rm)
        if [[ -e "$DEV" ]]; then
            v4l2loopback-ctl delete "$DEV"
            echo "[v4l2_gl310] Deleted $DEV."
        else
            echo "[v4l2_gl310] Device $DEV does not exist."
        fi
        ;;
    status|list)
        v4l2loopback-ctl list
        ;;
    -h|--help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
