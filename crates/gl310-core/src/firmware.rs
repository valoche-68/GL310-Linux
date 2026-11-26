//! Firmware discovery, strict SHA-256 verification, and safe import (directory or ZIP)
//! for AVerMedia GL310 (`qpvidfwusb.bin` and `qpaudfwusb.bin`).
//!
//! Proprietary AVerMedia binaries are NEVER embedded in or committed to this repository.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VIDEO_FW_FILENAME: &str = "qpvidfwusb.bin";
pub const AUDIO_FW_FILENAME: &str = "qpaudfwusb.bin";

pub const EXPECTED_VIDEO_FW_SHA256: &str =
    "8da30f6dc2d4fee5aeb3e2e06df5581db4c01d146c50c21f6a5d4824025a47d1";
pub const EXPECTED_AUDIO_FW_SHA256: &str =
    "0e8ea66b6a3ddb68f9a283f10155df90ce8d6f46599ed0ecf6f308d6c90210f4";

pub const OFFICIAL_ARCHIVE_URL: &str =
    "https://archive.org/download/aver-media-gl-310-usb-hd-capture-device-v-3.2802.64.40/AVerMedia%20GL310%20USB%20HD%20Capture%20Device%20v3.2802.64.40.zip";
pub const OFFICIAL_ARCHIVE_MD5: &str = "6aa363d7d7458dd08f021277353d6f2f";
pub const OFFICIAL_ARCHIVE_SHA256: &str =
    "894a2328291e6d7deffc79e4f56b3dd1dcbcb05c38094811b49de58c41b80229";

/// Verified in-memory firmware pair ready to upload into GL310 ARM RAM.
#[derive(Clone, Debug)]
pub struct VerifiedFirmware {
    pub directory: PathBuf,
    pub video_bytes: Vec<u8>,
    pub audio_bytes: Vec<u8>,
    pub video_sha256: String,
    pub audio_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FirmwareStatus {
    pub found: bool,
    pub valid: bool,
    pub directory: Option<String>,
    pub video_sha256: Option<String>,
    pub audio_sha256: Option<String>,
    pub video_size: Option<usize>,
    pub audio_size: Option<usize>,
    pub searched_paths: Vec<String>,
    pub message: String,
}

/// Default per-user XDG firmware directory (`~/.local/share/gl310-linux/firmware`).
pub fn user_firmware_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("gl310-linux").join("firmware");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("gl310-linux")
            .join("firmware");
    }
    PathBuf::from("./firmware")
}

/// Ordered candidate firmware directories.
pub fn candidate_firmware_dirs(explicit_override: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = explicit_override {
        dirs.push(p.to_path_buf());
    }
    if let Ok(env_dir) = std::env::var("GL310_FIRMWARE") {
        if !env_dir.trim().is_empty() {
            dirs.push(PathBuf::from(env_dir));
        }
    }
    dirs.push(PathBuf::from("./firmware"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("firmware"));
            if let Some(parent) = dir.parent() {
                dirs.push(parent.join("firmware"));
                if let Some(grand) = parent.parent() {
                    dirs.push(grand.join("firmware"));
                }
            }
        }
    }
    dirs.push(user_firmware_dir());
    dirs.push(PathBuf::from("/usr/share/gl310-linux/firmware"));
    dirs.push(PathBuf::from("/lib/firmware/avermedia"));

    // Deduplicate while preserving priority order.
    let mut unique = Vec::new();
    for d in dirs {
        if !unique.contains(&d) {
            unique.push(d);
        }
    }
    unique
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Read and strictly verify both firmware files in `dir`.
pub fn load_verified_from_dir(dir: &Path) -> Result<VerifiedFirmware, String> {
    let vid_path = dir.join(VIDEO_FW_FILENAME);
    let aud_path = dir.join(AUDIO_FW_FILENAME);

    let video_bytes = fs::read(&vid_path).map_err(|e| {
        format!(
            "Cannot read video firmware {}: {e}",
            vid_path.display()
        )
    })?;
    let audio_bytes = fs::read(&aud_path).map_err(|e| {
        format!(
            "Cannot read audio firmware {}: {e}",
            aud_path.display()
        )
    })?;

    verify_firmware_bytes(&video_bytes, &audio_bytes, dir.to_path_buf())
}

/// Strictly validate SHA-256 digests and 32-bit word alignment before any USB upload.
pub fn verify_firmware_bytes(
    video_bytes: &[u8],
    audio_bytes: &[u8],
    directory: PathBuf,
) -> Result<VerifiedFirmware, String> {
    if !video_bytes.len().is_multiple_of(4) || !audio_bytes.len().is_multiple_of(4) {
        return Err("Firmware size is not 4-byte aligned".to_string());
    }
    let video_sha256 = sha256_hex(video_bytes);
    let audio_sha256 = sha256_hex(audio_bytes);

    if video_sha256 != EXPECTED_VIDEO_FW_SHA256 {
        return Err(format!(
            "SHA-256 mismatch for {VIDEO_FW_FILENAME}: expected {EXPECTED_VIDEO_FW_SHA256}, got {video_sha256}"
        ));
    }
    if audio_sha256 != EXPECTED_AUDIO_FW_SHA256 {
        return Err(format!(
            "SHA-256 mismatch for {AUDIO_FW_FILENAME}: expected {EXPECTED_AUDIO_FW_SHA256}, got {audio_sha256}"
        ));
    }

    Ok(VerifiedFirmware {
        directory,
        video_bytes: video_bytes.to_vec(),
        audio_bytes: audio_bytes.to_vec(),
        video_sha256,
        audio_sha256,
    })
}

/// Locate and strictly verify the firmware pair across all configured search paths.
pub fn discover_verified_firmware(
    explicit_override: Option<&Path>,
) -> Result<VerifiedFirmware, String> {
    let candidates = candidate_firmware_dirs(explicit_override);
    let mut last_err = None;
    for dir in &candidates {
        let vid = dir.join(VIDEO_FW_FILENAME);
        let aud = dir.join(AUDIO_FW_FILENAME);
        if vid.is_file() && aud.is_file() {
            match load_verified_from_dir(dir) {
                Ok(fw) => return Ok(fw),
                Err(e) => last_err = Some(e),
            }
        }
    }
    if let Some(err) = last_err {
        return Err(err);
    }
    let searched = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "GL310 firmware files ({VIDEO_FW_FILENAME}, {AUDIO_FW_FILENAME}) not found. Searched: [{searched}]. Run `gl310-cli firmware fetch` or `gl310-cli firmware import <path>`."
    ))
}

/// Inspect firmware availability without failing, returning a structured diagnostic summary.
/// Searches all candidate paths consistently with `discover_verified_firmware` (if an earlier
/// path contains corrupted files, later valid paths are still checked before reporting invalid).
pub fn inspect_firmware_status(explicit_override: Option<&Path>) -> FirmwareStatus {
    let candidates = candidate_firmware_dirs(explicit_override);
    let searched_paths: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    let mut first_invalid: Option<FirmwareStatus> = None;

    for dir in &candidates {
        let vid = dir.join(VIDEO_FW_FILENAME);
        let aud = dir.join(AUDIO_FW_FILENAME);
        if vid.is_file() && aud.is_file() {
            match load_verified_from_dir(dir) {
                Ok(fw) => {
                    return FirmwareStatus {
                        found: true,
                        valid: true,
                        directory: Some(dir.display().to_string()),
                        video_sha256: Some(fw.video_sha256),
                        audio_sha256: Some(fw.audio_sha256),
                        video_size: Some(fw.video_bytes.len()),
                        audio_size: Some(fw.audio_bytes.len()),
                        searched_paths,
                        message: format!("Verified SHA-256 firmware pair in {}", dir.display()),
                    };
                }
                Err(e) => {
                    if first_invalid.is_none() {
                        first_invalid = Some(FirmwareStatus {
                            found: true,
                            valid: false,
                            directory: Some(dir.display().to_string()),
                            video_sha256: fs::read(&vid).ok().map(|b| sha256_hex(&b)),
                            audio_sha256: fs::read(&aud).ok().map(|b| sha256_hex(&b)),
                            video_size: fs::metadata(&vid).ok().map(|m| m.len() as usize),
                            audio_size: fs::metadata(&aud).ok().map(|m| m.len() as usize),
                            searched_paths: searched_paths.clone(),
                            message: e,
                        });
                    }
                }
            }
        }
    }

    if let Some(inv) = first_invalid {
        return inv;
    }

    FirmwareStatus {
        found: false,
        valid: false,
        directory: None,
        video_sha256: None,
        audio_sha256: None,
        video_size: None,
        audio_size: None,
        searched_paths,
        message: "Firmware not installed yet. Run `gl310-cli firmware fetch` or import a Windows driver ZIP/folder.".to_string(),
    }
}

/// Import `qpvidfwusb.bin` and `qpaudfwusb.bin` from either:
/// - a local directory (searched recursively up to depth 5, case-insensitively), or
/// - a local `.zip` Windows driver archive (read safely in-memory without executing anything).
///
/// Verifies strict SHA-256 digests before installing into `dest_dir` (defaults to `~/.local/share/gl310-linux/firmware`).
pub fn import_firmware_from_path(
    source: &Path,
    dest_dir: Option<&Path>,
) -> Result<VerifiedFirmware, String> {
    let target_dir = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(user_firmware_dir);

    let (video_bytes, audio_bytes) = if source.is_dir() {
        let vid_path = find_file_case_insensitive(source, VIDEO_FW_FILENAME, 5).ok_or_else(|| {
            format!(
                "Could not find {VIDEO_FW_FILENAME} inside directory {}",
                source.display()
            )
        })?;
        let aud_path = find_file_case_insensitive(source, AUDIO_FW_FILENAME, 5).ok_or_else(|| {
            format!(
                "Could not find {AUDIO_FW_FILENAME} inside directory {}",
                source.display()
            )
        })?;
        let vb = fs::read(&vid_path)
            .map_err(|e| format!("Failed reading {}: {e}", vid_path.display()))?;
        let ab = fs::read(&aud_path)
            .map_err(|e| format!("Failed reading {}: {e}", aud_path.display()))?;
        (vb, ab)
    } else if source.is_file() {
        extract_firmware_from_zip(source)?
    } else {
        return Err(format!("Source path does not exist: {}", source.display()));
    };

    let verified = verify_firmware_bytes(&video_bytes, &audio_bytes, target_dir.clone())?;

    fs::create_dir_all(&target_dir).map_err(|e| {
        format!(
            "Failed to create firmware destination {}: {e}",
            target_dir.display()
        )
    })?;
    fs::write(target_dir.join(VIDEO_FW_FILENAME), &verified.video_bytes)
        .map_err(|e| format!("Failed to write {VIDEO_FW_FILENAME}: {e}"))?;
    fs::write(target_dir.join(AUDIO_FW_FILENAME), &verified.audio_bytes)
        .map_err(|e| format!("Failed to write {AUDIO_FW_FILENAME}: {e}"))?;

    Ok(verified)
}

fn find_file_case_insensitive(root: &Path, target_lower: &str, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.eq_ignore_ascii_case(target_lower) {
                    return Some(path);
                }
            }
        } else if path.is_dir() {
            if let Some(found) = find_file_case_insensitive(&path, target_lower, max_depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn extract_firmware_from_zip(zip_path: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let file = fs::File::open(zip_path)
        .map_err(|e| format!("Cannot open archive {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Invalid ZIP archive {}: {e}", zip_path.display()))?;

    let mut vid_bytes: Option<Vec<u8>> = None;
    let mut aud_bytes: Option<Vec<u8>> = None;

    const MAX_FW_BYTES: u64 = 4 * 1024 * 1024;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed reading ZIP entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        if name.starts_with('/')
            || name.starts_with('\\')
            || name.contains("..")
            || name.contains('\\')
            || entry.enclosed_name().is_none()
        {
            return Err(format!("Unsafe ZIP path rejected: {name}"));
        }
        let base = Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if base.eq_ignore_ascii_case(VIDEO_FW_FILENAME) {
            if entry.size() > MAX_FW_BYTES {
                return Err(format!("Unexpectedly large {VIDEO_FW_FILENAME} in ZIP"));
            }
            let mut buf = Vec::with_capacity(entry.size().min(MAX_FW_BYTES) as usize);
            entry
                .take(MAX_FW_BYTES + 1)
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed extracting {VIDEO_FW_FILENAME}: {e}"))?;
            if buf.len() as u64 > MAX_FW_BYTES {
                return Err(format!("Decompressed {VIDEO_FW_FILENAME} exceeds safety limit"));
            }
            vid_bytes = Some(buf);
        } else if base.eq_ignore_ascii_case(AUDIO_FW_FILENAME) {
            if entry.size() > MAX_FW_BYTES {
                return Err(format!("Unexpectedly large {AUDIO_FW_FILENAME} in ZIP"));
            }
            let mut buf = Vec::with_capacity(entry.size().min(MAX_FW_BYTES) as usize);
            entry
                .take(MAX_FW_BYTES + 1)
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed extracting {AUDIO_FW_FILENAME}: {e}"))?;
            if buf.len() as u64 > MAX_FW_BYTES {
                return Err(format!("Decompressed {AUDIO_FW_FILENAME} exceeds safety limit"));
            }
            aud_bytes = Some(buf);
        }
    }

    match (vid_bytes, aud_bytes) {
        (Some(v), Some(a)) => Ok((v, a)),
        _ => Err(format!(
            "Archive {} does not contain both {VIDEO_FW_FILENAME} and {AUDIO_FW_FILENAME}",
            zip_path.display()
        )),
    }
}

/// Download the official AVerMedia v3.2802.64.40 archive via HTTPS (`curl`), verify its MD5,
/// extract both firmware files safely in memory (zero Windows binary execution), verify strict
/// SHA-256 digests, and install them into `dest_dir` (defaults to `~/.local/share/gl310-linux/firmware`).
pub fn fetch_and_install_official_firmware(
    dest_dir: Option<&Path>,
) -> Result<(VerifiedFirmware, String), String> {
    let target_dir = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(user_firmware_dir);
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create {}: {e}", target_dir.display()))?;

    let tmp_zip = target_dir.join(".gl310_driver_download.zip.part");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--retry",
            "3",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--silent",
            "--show-error",
            "-o",
        ])
        .arg(&tmp_zip)
        .arg(OFFICIAL_ARCHIVE_URL)
        .status()
        .map_err(|e| format!("Failed to launch curl for firmware download: {e}"))?;

    if !status.success() {
        let _ = fs::remove_file(&tmp_zip);
        return Err(format!(
            "curl failed downloading official driver archive (exit {:?})",
            status.code()
        ));
    }

    // Always enforce pure-Rust SHA-256 verification of the official archive (cannot be bypassed
    // even if external md5sum is missing), plus MD5 cross-check when md5sum is available.
    let mut audit_log = String::new();
    let zip_bytes = fs::read(&tmp_zip).map_err(|e| {
        let _ = fs::remove_file(&tmp_zip);
        format!("Failed reading downloaded archive: {e}")
    })?;
    let actual_zip_sha256 = sha256_hex(&zip_bytes);
    if actual_zip_sha256 != OFFICIAL_ARCHIVE_SHA256 {
        let _ = fs::remove_file(&tmp_zip);
        return Err(format!(
            "Archive SHA-256 mismatch! Expected {OFFICIAL_ARCHIVE_SHA256}, got {actual_zip_sha256}. Aborted."
        ));
    }
    audit_log.push_str(&format!("ZIP SHA-256 verified: {actual_zip_sha256}\n"));

    if let Ok(out) = Command::new("md5sum").arg(&tmp_zip).output() {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let actual_md5 = stdout.split_whitespace().next().unwrap_or("");
            if actual_md5 != OFFICIAL_ARCHIVE_MD5 {
                let _ = fs::remove_file(&tmp_zip);
                return Err(format!(
                    "Archive MD5 mismatch! Expected {OFFICIAL_ARCHIVE_MD5}, got {actual_md5}. Aborted."
                ));
            }
            audit_log.push_str(&format!("ZIP MD5 verified: {actual_md5}\n"));
        }
    }

    let import_res = import_firmware_from_path(&tmp_zip, Some(&target_dir));
    let _ = fs::remove_file(&tmp_zip);
    let fw = import_res?;
    audit_log.push_str(&format!(
        "{VIDEO_FW_FILENAME} SHA-256 verified: {}\n{AUDIO_FW_FILENAME} SHA-256 verified: {}\nInstalled to: {}",
        fw.video_sha256,
        fw.audio_sha256,
        fw.directory.display()
    ));
    Ok((fw, audit_log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_corrupted_or_unaligned_firmware() {
        assert!(verify_firmware_bytes(&[1, 2, 3], &[0, 0, 0, 0], PathBuf::from(".")).is_err());
        let bad = vec![0u8; 64];
        let err = verify_firmware_bytes(&bad, &bad, PathBuf::from(".")).unwrap_err();
        assert!(err.contains("SHA-256 mismatch"));
    }

    #[test]
    fn zip_traversal_is_rejected() {
        let tmp = std::env::temp_dir().join("gl310_test_traversal.zip");
        {
            let f = fs::File::create(&tmp).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("../evil.bin", opts).unwrap();
            zw.write_all(b"nope").unwrap();
            zw.finish().unwrap();
        }
        let err = extract_firmware_from_zip(&tmp).unwrap_err();
        let _ = fs::remove_file(&tmp);
        assert!(err.contains("Unsafe ZIP path"));
    }

    #[test]
    fn candidate_paths_include_override_first() {
        let custom = PathBuf::from("/tmp/custom-gl310-fw");
        let dirs = candidate_firmware_dirs(Some(&custom));
        assert_eq!(dirs[0], custom);
        assert!(dirs.iter().any(|p| p == Path::new("/lib/firmware/avermedia")));
    }
}
