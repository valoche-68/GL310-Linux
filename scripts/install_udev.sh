#!/usr/bin/env bash
# Installs the udev rule granting non-root USB access to AVerMedia GL310 (07ca:c835).
# Must be run with root privileges (e.g. `sudo bash scripts/install_udev.sh`).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$REPO_ROOT/udev/70-avermedia-gl310.rules"
DST="/etc/udev/rules.d/70-avermedia-gl310.rules"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: Please run with sudo: sudo bash $0" >&2
    exit 1
fi

install -m 0644 "$SRC" "$DST"
udevadm control --reload-rules
udevadm trigger --subsystem-match=usb --attr-match=idVendor=07ca

echo "[OK] Installed $DST and reloaded udev rules for USB 07ca:c835."
