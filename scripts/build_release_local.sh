#!/usr/bin/env bash
# Complete local release builder for GL310-Linux (zero GitHub Actions usage):
#   1. Anti-leak audit, unit tests, and Clippy
#   2. Release build of gl310-cli & gl310-studio
#   3. Generates clean dist/ artifacts (zero proprietary AVerMedia files):
#      - gl310-linux_<version>_amd64.deb
#      - GL310-Studio-<version>-x86_64.AppImage
#      - gl310-cli-v<version>-linux-x86_64.tar.gz
#      - SHA256SUMS.txt
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
ARCH="$(uname -m)"
DEB_ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
DIST_DIR="$REPO_ROOT/dist"

echo "=== [1/5] Anti-leak audit, syntax checks, tests & Clippy ==="
bash scripts/check_repo_clean.sh
node --check apps/gl310-studio/ui/app.js
bash -n install.sh scripts/*.sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

echo "=== [2/5] Building release binaries (gl310-cli & gl310-studio) ==="
cargo build --release -p gl310-cli -p gl310-studio

mkdir -p "$DIST_DIR"
rm -f "$DIST_DIR/SHA256SUMS.txt"

echo "=== [3/5] Packaging Debian .deb ==="
bash scripts/package_deb.sh

echo "=== [4/5] Packaging multi-distro .AppImage & standalone CLI tarball ==="
bash scripts/package_appimage.sh

CLI_STAGE="$DIST_DIR/gl310-cli-v${VERSION}-linux-${ARCH}"
CLI_TAR="$DIST_DIR/gl310-cli-v${VERSION}-linux-${ARCH}.tar.gz"
rm -rf "$CLI_STAGE" "$CLI_TAR"
mkdir -p "$CLI_STAGE/udev" "$CLI_STAGE/systemd"
install -m 0755 target/release/gl310-cli "$CLI_STAGE/gl310-cli"
install -m 0644 udev/70-avermedia-gl310.rules "$CLI_STAGE/udev/70-avermedia-gl310.rules"
install -m 0644 systemd/gl310-kvm.service "$CLI_STAGE/systemd/gl310-kvm.service"
install -m 0644 LICENSE "$CLI_STAGE/LICENSE"
install -m 0644 README.md "$CLI_STAGE/README.md"
install -m 0644 README.fr.md "$CLI_STAGE/README.fr.md"
tar --owner=0 --group=0 -C "$DIST_DIR" -czf "$CLI_TAR" "$(basename "$CLI_STAGE")"
rm -rf "$CLI_STAGE"

echo "=== [5/5] Verifying artifacts & writing SHA256SUMS.txt ==="
DEB_FILE="gl310-linux_${VERSION}_${DEB_ARCH}.deb"
APPIMAGE_FILE="GL310-Studio-${VERSION}-${ARCH}.AppImage"
CLI_TAR_FILE="gl310-cli-v${VERSION}-linux-${ARCH}.tar.gz"

(
    cd "$DIST_DIR"
    sha256sum "$DEB_FILE" "$APPIMAGE_FILE" "$CLI_TAR_FILE" > SHA256SUMS.txt
    sha256sum -c SHA256SUMS.txt
)

echo ""
echo "[build_release_local] All release artifacts ready in $DIST_DIR:"
ls -lh "$DIST_DIR/$DEB_FILE" "$DIST_DIR/$APPIMAGE_FILE" "$DIST_DIR/$CLI_TAR_FILE" "$DIST_DIR/SHA256SUMS.txt"
