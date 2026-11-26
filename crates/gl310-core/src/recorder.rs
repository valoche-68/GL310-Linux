//! Local `.ts` recorder with automatic `.ts -> .mp4` remux (`ffmpeg -c copy -movflags +faststart`)
//! and JPEG snapshot capture.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::stream::StreamHub;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub active: bool,
    pub path: Option<String>,
    pub bytes: u64,
    pub duration_secs: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub final_path: String,
    pub bytes: u64,
    pub duration_secs: f64,
    pub remuxed_mp4: bool,
    pub note: String,
}

pub struct TsRecorder {
    stop: Arc<AtomicBool>,
    bytes_written: Arc<AtomicU64>,
    started_at: Instant,
    ts_path: PathBuf,
    target_mp4_path: Option<PathBuf>,
    thread: Option<JoinHandle<()>>,
}

impl TsRecorder {
    /// Start recording MPEG-TS cleanly starting on the next PAT/PMT/IDR keyframe.
    /// If `output_path` ends with `.mp4`, records to a sibling `.ts` first and remuxes automatically on stop.
    pub fn start(hub: &StreamHub, output_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create recording directory {}: {e}",
                        parent.display()
                    )
                })?;
            }
        }

        let want_mp4 = output_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false);

        let (ts_path, target_mp4_path) = if want_mp4 {
            let ts = output_path.with_extension("ts");
            (ts, Some(output_path))
        } else {
            (output_path, None)
        };

        let mut file = File::create(&ts_path)
            .map_err(|e| format!("Failed to create recording file {}: {e}", ts_path.display()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let bytes_written = Arc::new(AtomicU64::new(0));
        let bytes_bg = Arc::clone(&bytes_written);
        let rx = hub.subscribe_ts(128, true);

        let thread = thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(chunk) => {
                        if file.write_all(&chunk).is_ok() {
                            bytes_bg.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                        } else {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }
            let _ = file.flush();
        });

        Ok(Self {
            stop,
            bytes_written,
            started_at: Instant::now(),
            ts_path,
            target_mp4_path,
            thread: Some(thread),
        })
    }

    /// Generate a timestamped `.mp4` path inside `recordings_dir`.
    pub fn default_mp4_path(recordings_dir: &Path) -> PathBuf {
        let stamp = Local::now().format("%Y%m%d_%H%M%S");
        recordings_dir.join(format!("gl310_{stamp}.mp4"))
    }

    pub fn status(&self) -> RecordingStatus {
        let display_path = self
            .target_mp4_path
            .as_ref()
            .unwrap_or(&self.ts_path)
            .display()
            .to_string();
        RecordingStatus {
            active: true,
            path: Some(display_path),
            bytes: self.bytes_written.load(Ordering::Relaxed),
            duration_secs: self.started_at.elapsed().as_secs_f64(),
        }
    }

    /// Stop recording and, when targeting `.mp4`, remux `.ts -> .mp4` losslessly with `+faststart`.
    pub fn stop(mut self) -> RecordingSummary {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        let duration_secs = self.started_at.elapsed().as_secs_f64();
        let bytes = self.bytes_written.load(Ordering::Relaxed);

        if let Some(mp4_path) = self.target_mp4_path.take() {
            if bytes == 0 {
                let _ = fs::remove_file(&self.ts_path);
                return RecordingSummary {
                    final_path: mp4_path.display().to_string(),
                    bytes: 0,
                    duration_secs,
                    remuxed_mp4: false,
                    note: "No video/audio packets were received during recording".into(),
                };
            }
            match remux_ts_to_mp4(&self.ts_path, &mp4_path) {
                Ok(mp4_bytes) => {
                    let _ = fs::remove_file(&self.ts_path);
                    return RecordingSummary {
                        final_path: mp4_path.display().to_string(),
                        bytes: mp4_bytes,
                        duration_secs,
                        remuxed_mp4: true,
                        note: "Remuxed losslessly to MP4 (+faststart)".into(),
                    };
                }
                Err(e) => {
                    return RecordingSummary {
                        final_path: self.ts_path.display().to_string(),
                        bytes,
                        duration_secs,
                        remuxed_mp4: false,
                        note: format!("Kept raw .ts (MP4 remux failed: {e})"),
                    };
                }
            }
        }

        RecordingSummary {
            final_path: self.ts_path.display().to_string(),
            bytes,
            duration_secs,
            remuxed_mp4: false,
            note: "Saved raw MPEG-TS".into(),
        }
    }
}

impl Drop for TsRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

/// Lossless `.ts -> .mp4` remux via `ffmpeg -c copy -movflags +faststart`.
pub fn remux_ts_to_mp4(ts_path: &Path, mp4_path: &Path) -> Result<u64, String> {
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-i",
        ])
        .arg(ts_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(mp4_path)
        .stdout(Stdio::null())
        .output()
        .map_err(|e| format!("Failed launching ffmpeg: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().to_string());
    }
    let meta = fs::metadata(mp4_path)
        .map_err(|e| format!("Output MP4 missing after remux: {e}"))?;
    if meta.len() == 0 {
        return Err("Output MP4 is empty".into());
    }
    Ok(meta.len())
}

/// Save the latest available JPEG frame from `StreamHub` into `recordings_dir`.
pub fn save_snapshot(hub: &StreamHub, recordings_dir: &Path) -> Result<PathBuf, String> {
    let jpeg = hub
        .latest_jpeg()
        .ok_or_else(|| "No video frame available yet for snapshot".to_string())?;
    fs::create_dir_all(recordings_dir).map_err(|e| {
        format!(
            "Failed creating snapshot directory {}: {e}",
            recordings_dir.display()
        )
    })?;
    let stamp = Local::now().format("%Y%m%d_%H%M%S");
    let path = recordings_dir.join(format!("gl310_snap_{stamp}.jpg"));
    fs::write(&path, jpeg.as_slice())
        .map_err(|e| format!("Failed writing snapshot {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::TsStats;

    #[test]
    fn recorder_writes_after_idr_and_saves_ts() {
        let hub = StreamHub::new();
        let tmp = std::env::temp_dir().join("gl310_test_rec.ts");
        let _ = fs::remove_file(&tmp);

        let rec = TsRecorder::start(&hub, tmp.clone()).unwrap();
        let pkt = vec![0x47u8; 188];
        // Pre-IDR ignored
        hub.publish_ts(pkt.clone(), false, &[], TsStats::default());
        // IDR triggers writing
        hub.publish_ts(pkt.clone(), true, &pkt, TsStats::default());
        thread::sleep(Duration::from_millis(80));
        let summary = rec.stop();
        assert!(summary.bytes >= 188);
        assert!(!summary.remuxed_mp4);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn snapshot_saves_latest_jpeg() {
        let hub = StreamHub::new();
        assert!(save_snapshot(&hub, &std::env::temp_dir()).is_err());

        hub.publish_jpeg(vec![0xFF, 0xD8, 0x01, 0x02, 0xFF, 0xD9]);
        let path = save_snapshot(&hub, &std::env::temp_dir()).unwrap();
        assert!(path.is_file());
        let data = fs::read(&path).unwrap();
        assert!(data.starts_with(&[0xFF, 0xD8]));
        let _ = fs::remove_file(&path);
    }
}
