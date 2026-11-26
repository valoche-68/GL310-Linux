#!/usr/bin/env bash
# Anti-leak audit script: verifies that no proprietary AVerMedia files,
# Windows driver artifacts, firmware binaries, or oversized binaries are
# tracked or staged in the Git repository or its commit history.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FORBIDDEN_REGEX='(qpvidfwusb|qpaudfwusb|aver330usb|\.(bin|sys|cat|inf|ax|exe|dll|zip|deb|appimage|rpm|ts|mp4)$|^vendor/|node_modules/)'
MAX_FILE_BYTES=$((2 * 1024 * 1024)) # 2 MiB guard for tracked source files

FAIL=0

echo "[check_repo_clean] Auditing repository: $REPO_ROOT"

if [[ ! -d .git ]]; then
    echo "ERROR: Not a Git repository." >&2
    exit 1
fi

# 1. Check tracked + staged files in the index
echo "[1/4] Checking tracked and staged files..."
TRACKED_HITS="$(git ls-files | grep -iE "$FORBIDDEN_REGEX" || true)"
if [[ -n "$TRACKED_HITS" ]]; then
    echo "FAIL: Forbidden proprietary/binary files found in Git index:" >&2
    echo "$TRACKED_HITS" >&2
    FAIL=1
fi

# 2. Check untracked non-ignored files that would be accidentally added by 'git add .'
echo "[2/4] Checking untracked (non-ignored) files..."
UNTRACKED_HITS="$(git ls-files --others --exclude-standard | grep -iE "$FORBIDDEN_REGEX" || true)"
if [[ -n "$UNTRACKED_HITS" ]]; then
    echo "FAIL: Unignored forbidden files present in working tree (check .gitignore):" >&2
    echo "$UNTRACKED_HITS" >&2
    FAIL=1
fi

# 3. Check entire Git history (if any commits exist)
echo "[3/4] Checking Git history..."
if git rev-parse --verify HEAD >/dev/null 2>&1; then
    HIST_HITS="$(git log --all --pretty=format: --name-only | sort -u | grep -iE "$FORBIDDEN_REGEX" || true)"
    if [[ -n "$HIST_HITS" ]]; then
        echo "FAIL: Forbidden files found in Git commit history:" >&2
        echo "$HIST_HITS" >&2
        FAIL=1
    fi
else
    echo "      (No commits yet — skipping history check)"
fi

# 4. Check file sizes and known firmware SHA-256 hashes across ALL tracked AND untracked non-ignored files
echo "[4/4] Checking file sizes and firmware digests (tracked + untracked non-ignored)..."
VID_SHA="8da30f6dc2d4fee5aeb3e2e06df5581db4c01d146c50c21f6a5d4824025a47d1"
AUD_SHA="0e8ea66b6a3ddb68f9a283f10155df90ce8d6f46599ed0ecf6f308d6c90210f4"
CHECKED_COUNT=0

while IFS= read -r -d '' file; do
    [[ -f "$file" ]] || continue
    CHECKED_COUNT=$((CHECKED_COUNT + 1))
    size="$(stat -c%s "$file")"
    if (( size > MAX_FILE_BYTES )); then
        echo "FAIL: Candidate Git file exceeds 2 MiB ($size bytes): $file" >&2
        FAIL=1
    fi
    digest="$(sha256sum "$file" | awk '{print $1}')"
    if [[ "$digest" == "$VID_SHA" || "$digest" == "$AUD_SHA" ]]; then
        echo "FAIL: Candidate Git file matches proprietary AVerMedia firmware SHA-256: $file" >&2
        FAIL=1
    fi
done < <(git ls-files -z; git ls-files --others --exclude-standard -z)
echo "      Audited $CHECKED_COUNT source/candidate files."

if (( FAIL != 0 )); then
    echo ""
    echo "[check_repo_clean] FAILED — DO NOT commit or push until resolved." >&2
    exit 1
fi

echo "[check_repo_clean] OK — Repository is clean (no proprietary AVerMedia artifacts or leaks)."
