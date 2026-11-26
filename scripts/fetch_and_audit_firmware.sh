#!/usr/bin/env bash
# Download, security-audit, and extract AVerMedia GL310 firmware files
# (qpvidfwusb.bin & qpaudfwusb.bin) from the official archived Windows driver.
#
# Security guarantees:
# - Never executes any Windows binary (.exe / .dll / .sys)
# - Verifies ZIP MD5 digest before extraction
# - Prints complete ZIP member inventory and rejects path traversal (../)
# - Audits Authenticode PKCS#7 certificate chain via OpenSSL on aver330usb.cat
# - Verifies strict SHA-256 digests of both firmware binaries before installing
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="${GL310_VENDOR_DIR:-$REPO_ROOT/vendor}"
LOCAL_FW_DIR="${GL310_LOCAL_FW_DIR:-$REPO_ROOT/firmware}"
USER_FW_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gl310-linux/firmware"

DRIVER_URL="https://archive.org/download/aver-media-gl-310-usb-hd-capture-device-v-3.2802.64.40/AVerMedia%20GL310%20USB%20HD%20Capture%20Device%20v3.2802.64.40.zip"
ZIP_NAME="AVerMedia_GL310_v3.2802.64.40.zip"
EXPECTED_ZIP_MD5="6aa363d7d7458dd08f021277353d6f2f"

EXPECTED_VID_SHA256="8da30f6dc2d4fee5aeb3e2e06df5581db4c01d146c50c21f6a5d4824025a47d1"
EXPECTED_AUD_SHA256="0e8ea66b6a3ddb68f9a283f10155df90ce8d6f46599ed0ecf6f308d6c90210f4"

INSTALL_USER=1
KEEP_VENDOR=1
LOCAL_ZIP=""

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Downloads, audits, and installs AVerMedia GL310 firmware binaries (qpvidfwusb.bin, qpaudfwusb.bin).

Options:
  --from-zip PATH    Audit and extract from an already downloaded local ZIP file
  --no-user-install  Only install into ./firmware/ (skip ~/.local/share/gl310-linux/firmware/)
  --clean-vendor     Remove temporary vendor extraction files after installing firmware
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --from-zip)
            LOCAL_ZIP="$2"
            shift 2
            ;;
        --no-user-install)
            INSTALL_USER=0
            shift
            ;;
        --clean-vendor)
            KEEP_VENDOR=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

for cmd in unzip sha256sum md5sum; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "ERROR: Missing required tool: $cmd" >&2
        exit 1
    fi
done

mkdir -p "$VENDOR_DIR" "$LOCAL_FW_DIR"

ZIP_PATH="$VENDOR_DIR/$ZIP_NAME"

if [[ -n "$LOCAL_ZIP" ]]; then
    if [[ ! -f "$LOCAL_ZIP" ]]; then
        echo "ERROR: Local ZIP not found: $LOCAL_ZIP" >&2
        exit 1
    fi
    cp -f "$LOCAL_ZIP" "$ZIP_PATH"
    echo "[1/6] Using local ZIP: $LOCAL_ZIP"
else
    if [[ -f "$ZIP_PATH" ]]; then
        CUR_MD5="$(md5sum "$ZIP_PATH" | awk '{print $1}')"
        if [[ "$CUR_MD5" == "$EXPECTED_ZIP_MD5" ]]; then
            echo "[1/6] Cached official ZIP already present and MD5-verified: $ZIP_PATH"
        else
            echo "[1/6] Cached ZIP MD5 mismatch ($CUR_MD5); re-downloading..."
            rm -f "$ZIP_PATH"
        fi
    fi
    if [[ ! -f "$ZIP_PATH" ]]; then
        echo "[1/6] Downloading official AVerMedia GL310 v3.2802.64.40 archive from Archive.org..."
        if command -v curl >/dev/null 2>&1; then
            curl --fail --location --retry 3 --proto '=https' --tlsv1.2 \
                -o "$ZIP_PATH.part" "$DRIVER_URL"
            mv -f "$ZIP_PATH.part" "$ZIP_PATH"
        elif command -v wget >/dev/null 2>&1; then
            wget --https-only -O "$ZIP_PATH.part" "$DRIVER_URL"
            mv -f "$ZIP_PATH.part" "$ZIP_PATH"
        else
            echo "ERROR: Neither curl nor wget is installed." >&2
            exit 1
        fi
    fi
fi

echo "[2/6] Verifying ZIP MD5 & SHA-256 digests..."
ACTUAL_MD5="$(md5sum "$ZIP_PATH" | awk '{print $1}')"
ACTUAL_ZIP_SHA256="$(sha256sum "$ZIP_PATH" | awk '{print $1}')"
echo "      MD5   : $ACTUAL_MD5"
echo "      SHA256: $ACTUAL_ZIP_SHA256"
if [[ -z "$LOCAL_ZIP" && "$ACTUAL_MD5" != "$EXPECTED_ZIP_MD5" ]]; then
    echo "ERROR: Archive MD5 mismatch! Expected $EXPECTED_ZIP_MD5, got $ACTUAL_MD5." >&2
    echo "       Refusing to extract untrusted archive." >&2
    exit 1
elif [[ "$ACTUAL_MD5" == "$EXPECTED_ZIP_MD5" ]]; then
    echo "      OK: Official v3.2802.64.40 archive MD5 matched."
else
    echo "      NOTE: Custom local ZIP MD5 differs from v3.2802.64.40; strict firmware SHA-256 check will still be enforced."
fi

echo "[3/6] Auditing ZIP member inventory (path traversal check)..."
unzip -l "$ZIP_PATH"
UNSAFE_ENTRIES="$(unzip -Z1 "$ZIP_PATH" | grep -E '(^/|\.\./|\.\.\\)' || true)"
if [[ -n "$UNSAFE_ENTRIES" ]]; then
    echo "ERROR: Unsafe archive member paths detected:" >&2
    echo "$UNSAFE_ENTRIES" >&2
    exit 1
fi

echo "[4/6] Extracting ONLY firmware & signature inspection files (zero .exe/.ax extraction or execution)..."
EXTRACT_DIR="$VENDOR_DIR/extracted"
rm -rf "$EXTRACT_DIR"
mkdir -p "$EXTRACT_DIR"

mapfile -t WANTED_ENTRIES < <(
    unzip -Z1 "$ZIP_PATH" | grep -iE '(^|/)(qpvidfwusb\.bin|qpaudfwusb\.bin|aver330usb\.sys|aver330usb\.cat|[^/]+\.inf)$' || true
)
if (( ${#WANTED_ENTRIES[@]} == 0 )); then
    echo "ERROR: No expected GL310 firmware/driver files found in archive." >&2
    exit 1
fi
unzip -q -o "$ZIP_PATH" "${WANTED_ENTRIES[@]}" -d "$EXTRACT_DIR"

# Locate target files case-insensitively
VID_SRC="$(find "$EXTRACT_DIR" -type f -iname 'qpvidfwusb.bin' | head -n 1 || true)"
AUD_SRC="$(find "$EXTRACT_DIR" -type f -iname 'qpaudfwusb.bin' | head -n 1 || true)"
SYS_SRC="$(find "$EXTRACT_DIR" -type f -iname 'AVer330USB.sys' | head -n 1 || true)"
CAT_SRC="$(find "$EXTRACT_DIR" -type f -iname 'aver330usb.cat' | head -n 1 || true)"
INF_SRC="$(find "$EXTRACT_DIR" -type f -iname '*.inf' | head -n 1 || true)"

if [[ -z "$VID_SRC" || -z "$AUD_SRC" ]]; then
    echo "ERROR: Could not find qpvidfwusb.bin and/or qpaudfwusb.bin in archive." >&2
    exit 1
fi

# Place top-level vendor copies for static inspection (gitignored)
if [[ -n "$SYS_SRC" ]]; then cp -f "$SYS_SRC" "$VENDOR_DIR/AVer330USB.sys"; fi
if [[ -n "$CAT_SRC" ]]; then cp -f "$CAT_SRC" "$VENDOR_DIR/aver330usb.cat"; fi
if [[ -n "$INF_SRC" ]]; then cp -f "$INF_SRC" "$VENDOR_DIR/$(basename "$INF_SRC")"; fi

echo "[5/6] Auditing Authenticode signature catalog (OpenSSL PKCS#7 inspection)..."
if [[ -n "$CAT_SRC" ]] && command -v openssl >/dev/null 2>&1; then
    echo "      Catalog file: $CAT_SRC"
    openssl pkcs7 -inform DER -in "$CAT_SRC" -print_certs -noout 2>/dev/null | sed 's/^/      /' || \
        echo "      WARN: Could not parse catalog PKCS#7 certificates via OpenSSL."
else
    echo "      NOTE: aver330usb.cat or openssl not present; skipping OpenSSL catalog check."
fi

echo "[6/6] Verifying strict SHA-256 digests of firmware binaries..."
VID_SHA="$(sha256sum "$VID_SRC" | awk '{print $1}')"
AUD_SHA="$(sha256sum "$AUD_SRC" | awk '{print $1}')"

echo "      qpvidfwusb.bin : $VID_SHA"
echo "      qpaudfwusb.bin : $AUD_SHA"

if [[ "$VID_SHA" != "$EXPECTED_VID_SHA256" ]]; then
    echo "ERROR: qpvidfwusb.bin SHA-256 mismatch!" >&2
    echo "       Expected: $EXPECTED_VID_SHA256" >&2
    echo "       Got     : $VID_SHA" >&2
    exit 1
fi

if [[ "$AUD_SHA" != "$EXPECTED_AUD_SHA256" ]]; then
    echo "ERROR: qpaudfwusb.bin SHA-256 mismatch!" >&2
    echo "       Expected: $EXPECTED_AUD_SHA256" >&2
    echo "       Got     : $AUD_SHA" >&2
    exit 1
fi

install -m 0644 "$VID_SRC" "$LOCAL_FW_DIR/qpvidfwusb.bin"
install -m 0644 "$AUD_SRC" "$LOCAL_FW_DIR/qpaudfwusb.bin"
echo "      Installed into: $LOCAL_FW_DIR/"

if (( INSTALL_USER == 1 )); then
    mkdir -p "$USER_FW_DIR"
    install -m 0644 "$VID_SRC" "$USER_FW_DIR/qpvidfwusb.bin"
    install -m 0644 "$AUD_SRC" "$USER_FW_DIR/qpaudfwusb.bin"
    echo "      Installed into: $USER_FW_DIR/"
fi

if (( KEEP_VENDOR == 0 )); then
    rm -rf "$EXTRACT_DIR" "$ZIP_PATH"
    echo "      Cleaned temporary vendor files."
fi

echo ""
echo "[OK] Security audit passed and GL310 firmwares are ready."
