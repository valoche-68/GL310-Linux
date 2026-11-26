//! Linux output sinks for GL310-Linux:
//! - V4L2 virtual camera (`/dev/video14` "AVerMedia GL310" via non-sudo `v4l2loopback-ctl`)
//! - PipeWire virtual audio source (`GL310 Audio`) + real-time stereo L/R VU-meter + live PC monitoring
//! - Built-in Rust HTTP server (`/stream.ts`, `/mjpeg`, `/snapshot.jpg`, `/health`)
//! - SRT listener (`:4850` via `ffmpeg -c copy -f mpegts`)
//! - Low-overhead H.264 -> MJPEG preview bridge for Studio UI when running in H.264+AAC mode

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::settings::{CaptureMode, Settings, V4l2FormatPolicy};
use crate::stream::{MjpegDeframer, StreamHub};

pub type LogSink = Arc<dyn Fn(&str, String) + Send + Sync>;

fn spawn_stderr_logger(tag: &'static str, stderr: Option<ChildStderr>, log: Option<LogSink>) {
    let Some(err) = stderr else {
        return;
    };
    thread::spawn(move || {
        let reader = BufReader::new(err);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(ref sink) = log {
                sink("warn", format!("[{tag}] {trimmed}"));
            }
        }
    });
}

/// Thread-safe handle allowing `Drop` / `stop()` to kill a subprocess immediately before joining
/// its feeder/reader thread, preventing deadlocks when a pipe (`stdin.write_all` / `stdout.read`)
/// is blocked (e.g., `ffmpeg` waiting for an SRT caller).
#[derive(Clone, Default)]
struct KillableChild {
    inner: Arc<Mutex<Option<Child>>>,
}

impl KillableChild {
    fn set(&self, child: Child) {
        let prev = self.inner.lock().unwrap().replace(child);
        if let Some(mut old) = prev {
            let _ = old.kill();
            let _ = old.wait();
        }
    }

    fn kill_and_reap(&self) {
        if let Some(mut ch) = self.inner.lock().unwrap().take() {
            let _ = ch.kill();
            let _ = ch.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Stereo VU-Meter Atomic State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct VuLevels {
    /// Left channel linear RMS (0.0 ..= 1.0)
    pub left: f32,
    /// Right channel linear RMS (0.0 ..= 1.0)
    pub right: f32,
    /// Left channel dBFS (-60.0 ..= 0.0)
    pub left_db: f32,
    /// Right channel dBFS (-60.0 ..= 0.0)
    pub right_db: f32,
}

#[derive(Clone, Default)]
pub struct VuMeter {
    left_bits: Arc<AtomicU32>,
    right_bits: Arc<AtomicU32>,
}

impl VuMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, left: f32, right: f32) {
        self.left_bits
            .store(left.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.right_bits
            .store(right.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn reset(&self) {
        self.update(0.0, 0.0);
    }

    pub fn read(&self) -> VuLevels {
        let l = f32::from_bits(self.left_bits.load(Ordering::Relaxed));
        let r = f32::from_bits(self.right_bits.load(Ordering::Relaxed));
        VuLevels {
            left: l,
            right: r,
            left_db: linear_to_db(l),
            right_db: linear_to_db(r),
        }
    }
}

fn linear_to_db(lin: f32) -> f32 {
    if lin <= 0.001 {
        -60.0
    } else {
        (20.0 * lin.log10()).clamp(-60.0, 0.0)
    }
}

/// Compute stereo L/R RMS from interleaved signed 16-bit little-endian PCM samples.
pub fn compute_pcm_s16le_vu(pcm: &[u8]) -> (f32, f32) {
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut frames = 0usize;
    for frame in pcm.chunks_exact(4) {
        let l = i16::from_le_bytes([frame[0], frame[1]]) as f64 / 32768.0;
        let r = i16::from_le_bytes([frame[2], frame[3]]) as f64 / 32768.0;
        sum_l += l * l;
        sum_r += r * r;
        frames += 1;
    }
    if frames == 0 {
        return (0.0, 0.0);
    }
    let rms_l = (sum_l / frames as f64).sqrt().clamp(0.0, 1.0) as f32;
    let rms_r = (sum_r / frames as f64).sqrt().clamp(0.0, 1.0) as f32;
    (rms_l, rms_r)
}

/// Live-adjustable PC audio monitoring state (shared between UI/CLI and PipeWire audio thread).
#[derive(Clone)]
pub struct MonitorControl {
    pub enabled: Arc<AtomicBool>,
    pub muted: Arc<AtomicBool>,
    pub volume: Arc<AtomicU8>,
}

impl MonitorControl {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(s.pc_monitor)),
            muted: Arc::new(AtomicBool::new(s.pc_monitor_muted)),
            volume: Arc::new(AtomicU8::new(s.pc_monitor_volume)),
        }
    }

    pub fn update(&self, enabled: bool, muted: bool, volume: u8) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.muted.store(muted, Ordering::Relaxed);
        self.volume.store(volume.min(100), Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// V4L2 Virtual Camera (/dev/video14 "AVerMedia GL310") — No sudo required
// ---------------------------------------------------------------------------

/// Ensure `/dev/videoN` exists using `v4l2loopback-ctl` without disturbing any existing nodes.
/// Returns `Ok(true)` if this call created the node (so we know to delete it on teardown),
/// or `Ok(false)` if it already existed.
pub fn ensure_v4l2_device(device: &str, label: &str) -> Result<bool, String> {
    if Path::new(device).exists() {
        return Ok(false);
    }
    let out = Command::new("v4l2loopback-ctl")
        .args(["add", "-n", label, "-x", "1", device])
        .output()
        .map_err(|e| format!("Failed to run v4l2loopback-ctl: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "v4l2loopback-ctl failed to create {device}: {}",
            stderr.trim()
        ));
    }
    // Wait briefly for systemd-udevd to apply `video` / `uaccess` permissions to the newly created
    // `/dev/videoN` node (otherwise opening immediately after `v4l2loopback-ctl add` races udev with EACCES).
    for _ in 0..25 {
        if OpenOptions::new().read(true).write(true).open(device).is_ok() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(40));
    }
    Ok(true)
}

pub fn remove_v4l2_device(device: &str) {
    let _ = Command::new("v4l2loopback-ctl")
        .args(["delete", device])
        .output();
}

/// Parse `(width, height)` from a JPEG frame's SOF0/SOF1/SOF2 marker (`0xFF 0xC0..=0xC2`).
pub fn parse_jpeg_dimensions(jpeg: &[u8]) -> Option<(u32, u32)> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut pos = 2usize;
    while pos + 3 < jpeg.len() {
        if jpeg[pos] != 0xFF {
            pos += 1;
            continue;
        }
        while pos + 1 < jpeg.len() && jpeg[pos + 1] == 0xFF {
            pos += 1;
        }
        if pos + 1 >= jpeg.len() {
            break;
        }
        let marker = jpeg[pos + 1];
        if matches!(marker, 0xD8 | 0xD9 | 0x00 | 0x01 | 0xD0..=0xD7) {
            pos += 2;
            continue;
        }
        if pos + 4 > jpeg.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC2) && pos + 9 <= jpeg.len() {
            let h = u16::from_be_bytes([jpeg[pos + 5], jpeg[pos + 6]]) as u32;
            let w = u16::from_be_bytes([jpeg[pos + 7], jpeg[pos + 8]]) as u32;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
        if marker == 0xDA || seg_len < 2 {
            break;
        }
        pos = pos.saturating_add(2 + seg_len);
    }
    None
}

#[cfg(target_os = "linux")]
fn v4l2_set_mjpeg_format(fd: std::os::fd::RawFd, width: u32, height: u32, sizeimage: u32) -> Result<(), String> {
    #[cfg(target_pointer_width = "64")]
    const FMT_SIZE: usize = 208;
    #[cfg(target_pointer_width = "64")]
    const PIX_OFF: usize = 8;
    #[cfg(target_pointer_width = "64")]
    const VIDIOC_S_FMT: libc::c_ulong = 0xC0D0_5605;

    #[cfg(target_pointer_width = "32")]
    const FMT_SIZE: usize = 204;
    #[cfg(target_pointer_width = "32")]
    const PIX_OFF: usize = 4;
    #[cfg(target_pointer_width = "32")]
    const VIDIOC_S_FMT: libc::c_ulong = 0xC0CC_5605;

    const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
    const V4L2_PIX_FMT_MJPEG: u32 = u32::from_le_bytes(*b"MJPG");
    const V4L2_FIELD_NONE: u32 = 1;
    const V4L2_COLORSPACE_JPEG: u32 = 7;

    let mut buf = [0u8; FMT_SIZE];
    buf[0..4].copy_from_slice(&V4L2_BUF_TYPE_VIDEO_OUTPUT.to_ne_bytes());
    buf[PIX_OFF..PIX_OFF + 4].copy_from_slice(&width.to_ne_bytes());
    buf[PIX_OFF + 4..PIX_OFF + 8].copy_from_slice(&height.to_ne_bytes());
    buf[PIX_OFF + 8..PIX_OFF + 12].copy_from_slice(&V4L2_PIX_FMT_MJPEG.to_ne_bytes());
    buf[PIX_OFF + 12..PIX_OFF + 16].copy_from_slice(&V4L2_FIELD_NONE.to_ne_bytes());
    // bytesperline = 0 for compressed MJPEG
    let sz = sizeimage.max(512 * 1024);
    buf[PIX_OFF + 20..PIX_OFF + 24].copy_from_slice(&sz.to_ne_bytes());
    buf[PIX_OFF + 24..PIX_OFF + 28].copy_from_slice(&V4L2_COLORSPACE_JPEG.to_ne_bytes());

    let rc = unsafe { libc::ioctl(fd, VIDIOC_S_FMT as _, buf.as_mut_ptr()) };
    if rc < 0 {
        return Err(format!(
            "V4L2 VIDIOC_S_FMT (MJPEG {}x{}) failed: {}",
            width,
            height,
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub struct V4l2Output {
    stop: Arc<AtomicBool>,
    child: KillableChild,
    device: String,
    created_device: bool,
    thread: Option<JoinHandle<()>>,
}

impl V4l2Output {
    pub fn start(
        hub: &StreamHub,
        mode: CaptureMode,
        device: String,
        format_policy: V4l2FormatPolicy,
    ) -> Result<Self, String> {
        Self::start_with_owned(hub, mode, device, format_policy, false)
    }

    /// Start V4L2 virtual camera output, optionally inheriting `created_device` ownership from a
    /// previous `V4l2Output::stop_keep_device` during an in-place reconfigure / HDMI resolution change.
    pub fn start_with_owned(
        hub: &StreamHub,
        mode: CaptureMode,
        device: String,
        format_policy: V4l2FormatPolicy,
        inherit_owned: bool,
    ) -> Result<Self, String> {
        Self::start_with_owned_logged(hub, mode, device, format_policy, inherit_owned, None)
    }

    pub fn start_with_owned_logged(
        hub: &StreamHub,
        mode: CaptureMode,
        device: String,
        format_policy: V4l2FormatPolicy,
        inherit_owned: bool,
        log: Option<LogSink>,
    ) -> Result<Self, String> {
        let created_device = if inherit_owned && Path::new(&device).exists() {
            true
        } else {
            ensure_v4l2_device(&device, "AVerMedia GL310")?
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let child_handle = KillableChild::default();

        let thread = match mode {
            CaptureMode::H264Aac => {
                let mut cmd = Command::new("ffmpeg");
                cmd.args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-fflags",
                    "+nobuffer+discardcorrupt",
                    "-flags",
                    "low_delay",
                    "-thread_type",
                    "slice",
                    "-threads",
                    "1",
                    "-probesize",
                    "65536",
                    "-analyzeduration",
                    "120000",
                    "-f",
                    "mpegts",
                    "-i",
                    "pipe:0",
                    "-an",
                ]);
                match format_policy {
                    V4l2FormatPolicy::Auto => {
                        cmd.args(["-fps_mode", "passthrough", "-flush_packets", "1", "-pix_fmt", "yuyv422"]);
                    }
                    V4l2FormatPolicy::Fixed1080p30 => {
                        cmd.args([
                            "-vf",
                            "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,fps=30",
                            "-fps_mode",
                            "passthrough",
                            "-flush_packets",
                            "1",
                            "-pix_fmt",
                            "yuyv422",
                        ]);
                    }
                }
                cmd.args(["-f", "v4l2", &device])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());

                let mut spawned = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        if created_device {
                            remove_v4l2_device(&device);
                        }
                        return Err(format!("Failed spawning ffmpeg for V4L2 {device}: {e}"));
                    }
                };
                let stderr = spawned.stderr.take();
                spawn_stderr_logger("v4l2-ffmpeg", stderr, log.clone());
                let mut stdin = spawned.stdin.take().ok_or_else(|| {
                    let _ = spawned.kill();
                    let _ = spawned.wait();
                    if created_device {
                        remove_v4l2_device(&device);
                    }
                    "Failed taking ffmpeg stdin for V4L2".to_string()
                })?;
                child_handle.set(spawned);
                let child_bg = child_handle.clone();
                let rx = hub.subscribe_ts(64, true);
                let dev_name = device.clone();

                thread::spawn(move || {
                    while !stop_bg.load(Ordering::Relaxed) {
                        match rx.recv_timeout(Duration::from_millis(150)) {
                            Ok(chunk) => {
                                if stdin.write_all(&chunk).is_err() {
                                    if !stop_bg.load(Ordering::Relaxed) {
                                        if let Some(ref sink) = log {
                                            sink(
                                                "warn",
                                                format!("V4L2 ffmpeg pipe closed unexpectedly on {dev_name}"),
                                            );
                                        }
                                    }
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        }
                    }
                    drop(stdin);
                    child_bg.kill_and_reap();
                })
            }
            CaptureMode::Mjpeg => {
                // True ZERO-ffmpeg pure-Rust direct V4L2 MJPEG injection (`VIDIOC_S_FMT` + `write()`).
                // Works on minimal Raspberry Pi KVM / PiKVM / uStreamer setups without ffmpeg installed!
                let mut v4l2_file = match OpenOptions::new().read(true).write(true).open(&device) {
                    Ok(f) => f,
                    Err(e) => {
                        if created_device {
                            remove_v4l2_device(&device);
                        }
                        return Err(format!("Failed opening V4L2 device {device} for MJPEG output: {e}"));
                    }
                };
                let rx = hub.subscribe_jpeg(8);
                thread::spawn(move || {
                    let mut configured_dims: Option<(u32, u32)> = None;
                    while !stop_bg.load(Ordering::Relaxed) {
                        match rx.recv_timeout(Duration::from_millis(150)) {
                            Ok(frame) => {
                                let dims = parse_jpeg_dimensions(&frame).unwrap_or((1280, 720));
                                if configured_dims != Some(dims) {
                                    #[cfg(target_os = "linux")]
                                    {
                                        if v4l2_set_mjpeg_format(
                                            v4l2_file.as_raw_fd(),
                                            dims.0,
                                            dims.1,
                                            frame.len() as u32,
                                        )
                                        .is_ok()
                                        {
                                            configured_dims = Some(dims);
                                        }
                                    }
                                    #[cfg(not(target_os = "linux"))]
                                    {
                                        configured_dims = Some(dims);
                                    }
                                }
                                if v4l2_file.write_all(&frame).is_err() {
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        }
                    }
                })
            }
        };

        Ok(Self {
            stop,
            child: child_handle,
            device,
            created_device,
            thread: Some(thread),
        })
    }

    /// Stop the V4L2 writer thread / subprocess WITHOUT deleting `/dev/videoN`, returning
    /// `Some(device)` if this instance created `/dev/videoN` so a restarted `V4l2Output`
    /// can inherit ownership cleanly (preventing EBUSY loss when OBS keeps the node open
    /// and avoiding unnecessary delete/add udev churn).
    pub fn stop_keep_device(mut self) -> Option<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        if self.created_device {
            self.created_device = false;
            Some(self.device.clone())
        } else {
            None
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        if self.created_device {
            remove_v4l2_device(&self.device);
            self.created_device = false;
        }
    }
}

impl Drop for V4l2Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        if self.created_device {
            remove_v4l2_device(&self.device);
            self.created_device = false;
        }
    }
}

// ---------------------------------------------------------------------------
// PipeWire Virtual Audio ('GL310 Audio') + Stereo VU-Meter + Live PC Monitoring
// ---------------------------------------------------------------------------

pub struct PipeWireAudioOutput {
    stop: Arc<AtomicBool>,
    dec_child: KillableChild,
    virtual_child: KillableChild,
    monitor_child: KillableChild,
    module_ids: Vec<u32>,
    thread: Option<JoinHandle<()>>,
    vu: VuMeter,
}

fn load_pactl_module(args: &[&str]) -> Option<u32> {
    let out = Command::new("pactl")
        .arg("load-module")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse::<u32>().ok()
}

fn unload_pactl_module(id: u32) {
    let _ = Command::new("pactl")
        .args(["unload-module", &id.to_string()])
        .output();
}

/// Remove any stale `gl310_audio_sink` or `gl310_audio_source` modules before creating fresh ones.
pub fn cleanup_stale_pipewire_modules() {
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains("gl310_audio_sink") || line.contains("gl310_audio_source") {
            if let Some(id_str) = line.split_whitespace().next() {
                if let Ok(id) = id_str.parse::<u32>() {
                    unload_pactl_module(id);
                }
            }
        }
    }
}

impl PipeWireAudioOutput {
    pub fn start(
        hub: &StreamHub,
        vu: VuMeter,
        monitor: MonitorControl,
    ) -> Result<Self, String> {
        cleanup_stale_pipewire_modules();
        let mut module_ids = Vec::new();

        // 1. Create null sink that receives decoded PCM from GL310.
        if let Some(sink_id) = load_pactl_module(&[
            "module-null-sink",
            "sink_name=gl310_audio_sink",
            "sink_properties=device.description=\"GL310_Audio_Bus\"",
            "rate=48000",
            "channels=2",
        ]) {
            module_ids.push(sink_id);

            // 2. Expose its monitor as a first-class virtual microphone source named "GL310 Audio" for OBS.
            // Note: PipeWire-pulse requires double quotes around the property map and single quotes around values with spaces.
            if let Some(src_id) = load_pactl_module(&[
                "module-remap-source",
                "master=gl310_audio_sink.monitor",
                "source_name=gl310_audio_source",
                "source_properties=\"device.description='GL310 Audio'\"",
            ]) {
                module_ids.push(src_id);
            }
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let rx = hub.subscribe_ts(64, true);
        let vu_bg = vu.clone();
        let has_sink = !module_ids.is_empty();

        let dec_child = KillableChild::default();
        let virtual_child = KillableChild::default();
        let monitor_child = KillableChild::default();

        let dec_bg = dec_child.clone();
        let virt_bg = virtual_child.clone();
        let mon_bg = monitor_child.clone();

        let thread = thread::spawn(move || {
            let mut dec = match Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-fflags",
                    "+nobuffer+discardcorrupt",
                    "-probesize",
                    "65536",
                    "-analyzeduration",
                    "120000",
                    "-f",
                    "mpegts",
                    "-i",
                    "pipe:0",
                    "-vn",
                    "-flush_packets",
                    "1",
                    "-f",
                    "s16le",
                    "-ar",
                    "48000",
                    "-ac",
                    "2",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return,
            };

            let mut dec_stdin = dec.stdin.take().unwrap();
            let mut dec_stdout = dec.stdout.take().unwrap();
            dec_bg.set(dec);
            let stop_feed = Arc::clone(&stop_bg);

            // Feeder thread: StreamHub MPEG-TS -> ffmpeg stdin
            let feeder = thread::spawn(move || {
                while !stop_feed.load(Ordering::Relaxed) {
                    match rx.recv_timeout(Duration::from_millis(150)) {
                        Ok(chunk) => {
                            if dec_stdin.write_all(&chunk).is_err() {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    }
                }
            });

            // Optional pw-cat feeding the virtual OBS source "GL310 Audio"
            let mut virtual_stdin = if has_sink {
                Command::new("pw-cat")
                    .args([
                        "--playback",
                        "--target",
                        "gl310_audio_sink",
                        "--raw",
                        "--format",
                        "s16",
                        "--rate",
                        "48000",
                        "--channels",
                        "2",
                        "--latency",
                        "40ms",
                        "-",
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()
                    .and_then(|mut ch| {
                        let sin = ch.stdin.take();
                        virt_bg.set(ch);
                        sin
                    })
            } else {
                None
            };

            // Live PC speaker monitoring process (started/stopped dynamically when toggled)
            let mut monitor_stdin: Option<std::process::ChildStdin> = None;

            let mut pcm_buf = vec![0u8; 3840]; // 20 ms @ 48 kHz stereo s16le (960 frames * 4 bytes)
            let mut scaled_buf = vec![0u8; 3840];

            while !stop_bg.load(Ordering::Relaxed) {
                match dec_stdout.read(&mut pcm_buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let valid = n - (n % 4);
                        if valid == 0 {
                            continue;
                        }
                        let slice = &pcm_buf[..valid];
                        let (l, r) = compute_pcm_s16le_vu(slice);
                        vu_bg.update(l, r);

                        if let Some(ref mut sin) = virtual_stdin {
                            if sin.write_all(slice).is_err() {
                                virtual_stdin = None;
                                virt_bg.kill_and_reap();
                            }
                        }

                        let mon_en = monitor.enabled.load(Ordering::Relaxed);
                        let mon_muted = monitor.muted.load(Ordering::Relaxed);
                        let mon_vol = monitor.volume.load(Ordering::Relaxed);

                        if mon_en && !mon_muted && mon_vol > 0 {
                            if monitor_stdin.is_none() {
                                monitor_stdin = Command::new("pw-cat")
                                    .args([
                                        "--playback",
                                        "--raw",
                                        "--format",
                                        "s16",
                                        "--rate",
                                        "48000",
                                        "--channels",
                                        "2",
                                        "--latency",
                                        "40ms",
                                        "-",
                                    ])
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()
                                    .ok()
                                    .and_then(|mut ch| {
                                        let sin = ch.stdin.take();
                                        mon_bg.set(ch);
                                        sin
                                    });
                            }
                            if let Some(ref mut sin) = monitor_stdin {
                                let gain = (mon_vol as f32) / 100.0;
                                for (i, pair) in slice.chunks_exact(2).enumerate() {
                                    let sample = i16::from_le_bytes([pair[0], pair[1]]) as f32;
                                    let out_s = (sample * gain).clamp(-32768.0, 32767.0) as i16;
                                    let bytes = out_s.to_le_bytes();
                                    scaled_buf[2 * i] = bytes[0];
                                    scaled_buf[2 * i + 1] = bytes[1];
                                }
                                if sin.write_all(&scaled_buf[..valid]).is_err() {
                                    monitor_stdin = None;
                                    mon_bg.kill_and_reap();
                                }
                            }
                        } else if monitor_stdin.take().is_some() {
                            mon_bg.kill_and_reap();
                        }
                    }
                }
            }

            vu_bg.reset();
            drop(virtual_stdin);
            drop(monitor_stdin);
            virt_bg.kill_and_reap();
            mon_bg.kill_and_reap();
            dec_bg.kill_and_reap();
            let _ = feeder.join();
        });

        Ok(Self {
            stop,
            dec_child,
            virtual_child,
            monitor_child,
            module_ids,
            thread: Some(thread),
            vu,
        })
    }
}

impl Drop for PipeWireAudioOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.dec_child.kill_and_reap();
        self.virtual_child.kill_and_reap();
        self.monitor_child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        self.vu.reset();
        for &id in self.module_ids.iter().rev() {
            unload_pactl_module(id);
        }
    }
}

// ---------------------------------------------------------------------------
// Studio Preview MJPEG Bridge (for H.264+AAC mode)
// ---------------------------------------------------------------------------

pub struct PreviewBridge {
    stop: Arc<AtomicBool>,
    child: KillableChild,
    thread: Option<JoinHandle<()>>,
}

impl PreviewBridge {
    pub fn start(hub: &StreamHub) -> Self {
        Self::start_logged(hub, None)
    }

    pub fn start_logged(hub: &StreamHub, log: Option<LogSink>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let rx = hub.subscribe_ts(48, true);
        let hub_bg = hub.clone();
        let child_handle = KillableChild::default();
        let child_bg = child_handle.clone();

        let thread = thread::spawn(move || {
            let mut child = match Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-fflags",
                    "+nobuffer+discardcorrupt",
                    "-flags",
                    "low_delay",
                    "-thread_type",
                    "slice",
                    "-threads",
                    "1",
                    "-probesize",
                    "65536",
                    "-analyzeduration",
                    "120000",
                    "-f",
                    "mpegts",
                    "-i",
                    "pipe:0",
                    "-an",
                    "-vf",
                    "scale=1280:-2",
                    "-fps_mode",
                    "passthrough",
                    "-flush_packets",
                    "1",
                    "-strict",
                    "unofficial",
                    "-q:v",
                    "6",
                    "-f",
                    "mjpeg",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    if let Some(ref sink) = log {
                        sink("error", format!("[preview-ffmpeg] failed to spawn: {e}"));
                    }
                    return;
                }
            };

            spawn_stderr_logger("preview-ffmpeg", child.stderr.take(), log.clone());
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            child_bg.set(child);
            let stop_feed = Arc::clone(&stop_bg);

            let feeder = thread::spawn(move || {
                while !stop_feed.load(Ordering::Relaxed) {
                    match rx.recv_timeout(Duration::from_millis(150)) {
                        Ok(chunk) => {
                            if stdin.write_all(&chunk).is_err() {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    }
                }
            });

            let mut deframer = MjpegDeframer::new();
            let mut buf = vec![0u8; 32_768];
            while !stop_bg.load(Ordering::Relaxed) {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for frame in deframer.push(&buf[..n]) {
                            hub_bg.publish_jpeg(frame);
                        }
                    }
                }
            }

            child_bg.kill_and_reap();
            let _ = feeder.join();
        });

        Self {
            stop,
            child: child_handle,
            thread: Some(thread),
        }
    }
}

impl Drop for PreviewBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// SRT Listener Output (:4850 via ffmpeg -c copy -f mpegts)
// ---------------------------------------------------------------------------

pub struct SrtOutput {
    stop: Arc<AtomicBool>,
    child: KillableChild,
    thread: Option<JoinHandle<()>>,
    pub url: String,
}

impl SrtOutput {
    pub fn start(hub: &StreamHub, bind_host: &str, port: u16) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let child_handle = KillableChild::default();
        let child_bg = child_handle.clone();
        let rx = hub.subscribe_ts(64, true);
        let listen_url = format!(
            "srt://{bind_host}:{port}?mode=listener&transtype=live&latency=120000"
        );
        let client_host = if bind_host == "0.0.0.0" {
            crate::display_host_for(true)
        } else {
            bind_host.to_string()
        };
        let client_url =
            format!("srt://{client_host}:{port}?mode=caller&transtype=live&latency=120000");

        let thread = thread::spawn(move || {
            // Re-spawn SRT listener cleanly whenever an OBS client disconnects so OBS can reconnect freely.
            while !stop_bg.load(Ordering::Relaxed) {
                let mut spawned = match Command::new("ffmpeg")
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-fflags",
                        "+genpts",
                        "-probesize",
                        "250000",
                        "-analyzeduration",
                        "300000",
                        "-f",
                        "mpegts",
                        "-i",
                        "pipe:0",
                        "-c",
                        "copy",
                        "-f",
                        "mpegts",
                        &listen_url,
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => break,
                };

                let stdin = spawned.stdin.take();
                child_bg.set(spawned);

                if let Some(mut stdin) = stdin {
                    while !stop_bg.load(Ordering::Relaxed) {
                        match rx.recv_timeout(Duration::from_millis(150)) {
                            Ok(chunk) => {
                                if stdin.write_all(&chunk).is_err() {
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => {
                                stop_bg.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                }
                child_bg.kill_and_reap();
                if !stop_bg.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(250));
                }
            }
        });

        Self {
            stop,
            child: child_handle,
            thread: Some(thread),
            url: client_url,
        }
    }
}

impl Drop for SrtOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Kill ffmpeg BEFORE joining: when no SRT caller is connected, ffmpeg blocks opening the
        // SRT listener socket, which fills pipe:0 and blocks stdin.write_all until the child dies.
        self.child.kill_and_reap();
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Built-In Rust HTTP Server (/stream.ts, /mjpeg, /snapshot.jpg, /health)
// ---------------------------------------------------------------------------

type ActiveHttpClients = Arc<Mutex<Vec<(u64, TcpStream)>>>;

struct HttpClientRegistration {
    id: u64,
    clients: ActiveHttpClients,
}

impl Drop for HttpClientRegistration {
    fn drop(&mut self) {
        if let Ok(mut list) = self.clients.lock() {
            list.retain(|(id, _)| *id != self.id);
        }
    }
}

pub struct HttpServer {
    stop: Arc<AtomicBool>,
    addr: String,
    clients: ActiveHttpClients,
    thread: Option<JoinHandle<()>>,
}

impl HttpServer {
    pub fn start(hub: StreamHub, bind_host: &str, port: u16) -> Result<Self, String> {
        let bind_addr = format!("{bind_host}:{port}");
        let listener = TcpListener::bind(&bind_addr)
            .map_err(|e| format!("Failed to bind HTTP server on {bind_addr}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed setting HTTP listener nonblocking: {e}"))?;

        let actual_addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| bind_addr.clone());

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let clients: ActiveHttpClients = Arc::new(Mutex::new(Vec::new()));
        let clients_bg = Arc::clone(&clients);

        let thread = thread::spawn(move || {
            let mut next_id: u64 = 1;
            while !stop_bg.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let reg = stream.try_clone().ok().map(|clone| {
                            let id = next_id;
                            next_id = next_id.wrapping_add(1);
                            if let Ok(mut list) = clients_bg.lock() {
                                list.push((id, clone));
                            }
                            HttpClientRegistration {
                                id,
                                clients: Arc::clone(&clients_bg),
                            }
                        });
                        if stop_bg.load(Ordering::Acquire) {
                            let _ = stream.shutdown(Shutdown::Both);
                            drop(reg);
                            break;
                        }
                        let hub_c = hub.clone();
                        let stop_c = Arc::clone(&stop_bg);
                        thread::spawn(move || handle_http_client(stream, hub_c, stop_c, reg));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            stop,
            addr: actual_addr,
            clients,
            thread: Some(thread),
        })
    }

    pub fn address(&self) -> &str {
        &self.addr
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        // Shut down all active client sockets immediately so OBS/browser streams receive prompt
        // EOF and never read post-restart chunks or wait for write/recv timeouts.
        if let Ok(list) = self.clients.lock() {
            for (_, s) in list.iter() {
                let _ = s.shutdown(Shutdown::Both);
            }
        }
    }
}

fn handle_http_client(
    mut stream: TcpStream,
    hub: StreamHub,
    stop: Arc<AtomicBool>,
    _reg: Option<HttpClientRegistration>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain remaining HTTP request headers cleanly so closing short-lived endpoints
    // (/health, /snapshot.jpg) never leaves unread bytes that trigger TCP RST in WebKit/libsoup.
    let mut header_line = String::new();
    for _ in 0..64 {
        header_line.clear();
        match reader.read_line(&mut header_line) {
            Ok(0) | Err(_) => break,
            Ok(_) if header_line == "\r\n" || header_line == "\n" => break,
            Ok(_) => {}
        }
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");

    match path {
        "/health" => {
            let stats = hub.ts_stats();
            let body = format!(
                "{{\"ok\":true,\"ts_packets\":{},\"ts_continuity_errors\":{},\"mjpeg_frames\":{},\"has_snapshot\":{}}}",
                stats.packets,
                stats.continuity_errors,
                hub.mjpeg_frames(),
                hub.latest_jpeg().is_some()
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
        "/snapshot.jpg" => {
            if let Some(jpeg) = hub.latest_jpeg() {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    jpeg.len()
                );
                let _ = stream.write_all(hdr.as_bytes());
                let _ = stream.write_all(&jpeg);
            } else {
                let resp = "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 17\r\nConnection: close\r\n\r\nNo video frame.\r\n";
                let _ = stream.write_all(resp.as_bytes());
            }
        }
        "/stream.ts" => {
            let hdr = "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
            if stream.write_all(hdr.as_bytes()).is_err() {
                return;
            }
            let rx = hub.subscribe_ts(64, true);
            while !stop.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(chunk) => {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if stream.write_all(&chunk).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }
        }
        "/mjpeg" | "/stream.mjpeg" => {
            let hdr = "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store, no-cache\r\nPragma: no-cache\r\nConnection: close\r\n\r\n";
            if stream.write_all(hdr.as_bytes()).is_err() {
                return;
            }
            let rx = hub.subscribe_jpeg(4);
            while !stop.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(jpeg) => {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let part_hdr = format!(
                            "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                            jpeg.len()
                        );
                        if stream.write_all(part_hdr.as_bytes()).is_err()
                            || stream.write_all(&jpeg).is_err()
                            || stream.write_all(b"\r\n").is_err()
                        {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }
        }
        "/" => {
            // Minimal built-in KVM viewer page (ideal for Raspberry Pi headless browser access!).
            let html = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>GL310 Live KVM</title>
<style>body{margin:0;background:#08090c;color:#e4e7ec;font-family:system-ui,sans-serif;display:flex;flex-direction:column;height:100vh}header{padding:8px 16px;background:#111318;border-bottom:1px solid #222631;display:flex;justify-content:space-between;align-items:center;font-size:13px}main{flex:1;display:flex;align-items:center;justify-content:center;overflow:hidden}img{max-width:100%;max-height:100%;object-fit:contain}a{color:#5eead4;text-decoration:none;margin-left:12px}</style>
</head>
<body>
<header><strong>AVerMedia GL310 — Live Stream</strong><div><a href="/snapshot.jpg" target="_blank">Snapshot</a><a href="/stream.ts">MPEG-TS</a><a href="/health">Health</a></div></header>
<main><img src="/mjpeg" alt="GL310 Live Stream"></main>
</body></html>"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                html.len(),
                html
            );
            let _ = stream.write_all(resp.as_bytes());
        }
        _ => {
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
            let _ = stream.write_all(resp.as_bytes());
        }
    }
}

/// Active output bundle owned by a running capture session.
pub struct ActiveOutputs {
    pub v4l2: Option<V4l2Output>,
    pub pipewire: Option<PipeWireAudioOutput>,
    pub preview: Option<PreviewBridge>,
    pub srt: Option<SrtOutput>,
    pub http: Option<HttpServer>,
    pub warnings: Mutex<Vec<String>>,
}

impl ActiveOutputs {
    pub fn start_all(
        settings: &Settings,
        hub: &StreamHub,
        vu: VuMeter,
        monitor: MonitorControl,
        start_preview_bridge: bool,
    ) -> Self {
        Self::start_all_with_owned(settings, hub, vu, monitor, start_preview_bridge, None)
    }

    /// Tear down all active sinks during a live reconfigure / HDMI mode switch while preserving
    /// ownership of any `/dev/videoN` node created by this session so it is neither needlessly
    /// deleted/recreated nor leaked if OBS still holds it open (`EBUSY`).
    pub fn take_owned_v4l2_device(mut self) -> Option<String> {
        self.v4l2.take().and_then(V4l2Output::stop_keep_device)
    }

    pub fn start_all_with_owned(
        settings: &Settings,
        hub: &StreamHub,
        vu: VuMeter,
        monitor: MonitorControl,
        start_preview_bridge: bool,
        owned_v4l2_device: Option<String>,
    ) -> Self {
        Self::start_all_with_owned_logged(
            settings,
            hub,
            vu,
            monitor,
            start_preview_bridge,
            owned_v4l2_device,
            None,
        )
    }

    pub fn start_all_with_owned_logged(
        settings: &Settings,
        hub: &StreamHub,
        vu: VuMeter,
        monitor: MonitorControl,
        start_preview_bridge: bool,
        owned_v4l2_device: Option<String>,
        log: Option<LogSink>,
    ) -> Self {
        let mut warnings = Vec::new();
        let bind_host = settings.bind_host();

        let http = if settings.enable_http {
            match HttpServer::start(hub.clone(), bind_host, settings.http_port) {
                Ok(s) => Some(s),
                Err(e) => {
                    warnings.push(e);
                    None
                }
            }
        } else {
            None
        };

        let mut inherit_v4l2 = false;
        if let Some(prev_dev) = owned_v4l2_device {
            if settings.enable_v4l2 && prev_dev == settings.v4l2_device {
                inherit_v4l2 = true;
            } else {
                remove_v4l2_device(&prev_dev);
            }
        }

        let v4l2 = if settings.enable_v4l2 {
            match V4l2Output::start_with_owned_logged(
                hub,
                settings.mode,
                settings.v4l2_device.clone(),
                settings.v4l2_format,
                inherit_v4l2,
                log.clone(),
            ) {
                Ok(v) => Some(v),
                Err(e) => {
                    warnings.push(e);
                    None
                }
            }
        } else {
            None
        };

        let pipewire = if settings.mode == CaptureMode::H264Aac && settings.enable_pipewire_audio {
            match PipeWireAudioOutput::start(hub, vu, monitor) {
                Ok(p) => Some(p),
                Err(e) => {
                    warnings.push(e);
                    None
                }
            }
        } else {
            None
        };

        let preview = if settings.mode == CaptureMode::H264Aac && start_preview_bridge {
            Some(PreviewBridge::start_logged(hub, log))
        } else {
            None
        };

        let srt = if settings.mode == CaptureMode::H264Aac && settings.enable_srt {
            Some(SrtOutput::start(hub, bind_host, settings.srt_port))
        } else {
            None
        };

        Self {
            v4l2,
            pipewire,
            preview,
            srt,
            http,
            warnings: Mutex::new(warnings),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vu_meter_computes_silence_and_full_scale() {
        let silence = vec![0u8; 400];
        let (l0, r0) = compute_pcm_s16le_vu(&silence);
        assert_eq!(l0, 0.0);
        assert_eq!(r0, 0.0);

        // Left = +16384 (~0.5), Right = -32768 (1.0)
        let mut buf = Vec::new();
        for _ in 0..100 {
            buf.extend_from_slice(&16384i16.to_le_bytes());
            buf.extend_from_slice(&(-32768i16).to_le_bytes());
        }
        let (l, r) = compute_pcm_s16le_vu(&buf);
        assert!((l - 0.5).abs() < 0.01);
        assert!((r - 1.0).abs() < 0.01);
    }

    #[test]
    fn http_server_serves_health_and_snapshot() {
        let hub = StreamHub::new();
        let mut fake_jpeg = vec![0xFF, 0xD8];
        fake_jpeg.extend_from_slice(&[0x42; 200]);
        fake_jpeg.extend_from_slice(&[0xFF, 0xD9]);
        hub.publish_jpeg(fake_jpeg.clone());

        let srv = HttpServer::start(hub, "127.0.0.1", 0).unwrap();
        let addr = srv.address().to_string();

        // Health endpoint
        let mut s = TcpStream::connect(&addr).unwrap();
        s.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200 OK"));
        assert!(resp.contains("\"has_snapshot\":true"));

        // Snapshot endpoint
        let mut s2 = TcpStream::connect(&addr).unwrap();
        s2.write_all(b"GET /snapshot.jpg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut raw = Vec::new();
        s2.read_to_end(&mut raw).unwrap();
        let sep = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        assert_eq!(&raw[sep + 4..], fake_jpeg.as_slice());

        // MJPEG stream endpoint delivers boundary + latest frame
        let mut s3 = TcpStream::connect(&addr).unwrap();
        s3.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s3.write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut buf = [0u8; 1024];
        let mut total = Vec::new();
        for _ in 0..4 {
            match s3.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total.extend_from_slice(&buf[..n]);
                    if total.windows(2).any(|w| w == [0xFF, 0xD9]) {
                        break;
                    }
                }
            }
        }
        let header_str = String::from_utf8_lossy(&total);
        assert!(header_str.contains("multipart/x-mixed-replace; boundary=frame"));
        assert!(total.windows(2).any(|w| w == [0xFF, 0xD8]));
        assert!(total.windows(2).any(|w| w == [0xFF, 0xD9]));
    }

    #[test]
    fn parse_jpeg_dimensions_reads_sof0_and_sof2() {
        // Minimal SOI + APP0 + SOF0 (1280 x 720) + EOI
        let jpeg = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x06, 0x4A, 0x46, 0x49, 0x46, // APP0 (len 6)
            0xFF, 0xC0, 0x00, 0x0B, 0x08, // SOF0, precision=8
            0x02, 0xD0, // height = 720
            0x05, 0x00, // width  = 1280
            0x01, 0x01, 0x11, 0x00,
            0xFF, 0xD9,
        ];
        assert_eq!(parse_jpeg_dimensions(&jpeg), Some((1280, 720)));
        assert_eq!(parse_jpeg_dimensions(&[0x00, 0x01]), None);
    }

    #[test]
    fn srt_output_drops_cleanly_without_deadlock_when_no_client_connects() {
        // Regression test: previously, pumping >64 KiB into SrtOutput while no SRT caller was
        // connected caused ffmpeg's pipe:0 to fill and SrtOutput::drop to deadlock forever.
        let hub = StreamHub::new();
        let srt = SrtOutput::start(&hub, "127.0.0.1", 14_859);

        // Publish >256 KiB of TS data with IDR flag so SrtOutput's feeder tries to write past pipe capacity
        let mut chunk = vec![0u8; 188 * 100]; // 18.8 KiB
        for pkt in chunk.chunks_exact_mut(188) {
            pkt[0] = 0x47;
        }
        for _ in 0..20 {
            hub.publish_ts(chunk.clone(), true, &chunk[..188], crate::stream::TsStats::default());
        }
        thread::sleep(Duration::from_millis(120));

        let started = std::time::Instant::now();
        drop(srt);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "SrtOutput::drop took {:?} — possible deadlock!",
            started.elapsed()
        );
    }

    #[test]
    fn http_server_restarts_cleanly_on_same_port_with_active_ts_client() {
        let hub = StreamHub::new();
        let srv1 = HttpServer::start(hub.clone(), "127.0.0.1", 0).unwrap();
        let addr = srv1.address().to_string();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();

        // Connect an OBS-style /stream.ts client and feed an IDR chunk
        let mut client1 = TcpStream::connect(&addr).unwrap();
        client1.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        client1
            .write_all(b"GET /stream.ts HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(60));
        let mut pkt = vec![0u8; 188];
        pkt[0] = 0x47;
        hub.publish_ts(pkt.clone(), true, &pkt, crate::stream::TsStats::default());

        let mut hdr_buf = [0u8; 256];
        let n = client1.read(&mut hdr_buf).unwrap();
        assert!(String::from_utf8_lossy(&hdr_buf[..n]).contains("200 OK"));

        // Simulate HDMI resolution change: drop old HttpServer and immediately rebind the same port
        drop(srv1);
        let srv2 = HttpServer::start(hub.clone(), "127.0.0.1", port)
            .expect("HttpServer must rebind cleanly on same port after restart");

        // Old client must observe EOF cleanly when srv1 stops (within timeout)
        let mut drain = [0u8; 1024];
        let mut saw_eof = false;
        for _ in 0..8 {
            match client1.read(&mut drain) {
                Ok(0) => {
                    saw_eof = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_eof, "Old HTTP /stream.ts client should disconnect cleanly on output restart");

        // New client connects to restarted server on the exact same port
        let mut client2 = TcpStream::connect(srv2.address()).unwrap();
        client2
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        client2.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200 OK"));
        drop(srv2);
    }

    #[test]
    fn http_server_drop_immediately_closes_old_client_without_leaking_post_restart_chunks() {
        let hub = StreamHub::new();
        let srv1 = HttpServer::start(hub.clone(), "127.0.0.1", 0).unwrap();
        let addr = srv1.address().to_string();
        let port: u16 = addr.rsplit(':').next().unwrap().parse().unwrap();

        let mut client1 = TcpStream::connect(&addr).unwrap();
        client1
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        client1
            .write_all(b"GET /stream.ts HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        thread::sleep(Duration::from_millis(40));

        // Publish pre-restart chunk marked with 0x11
        let mut pre_pkt = vec![0x11u8; 188];
        pre_pkt[0] = 0x47;
        hub.publish_ts(
            pre_pkt.clone(),
            true,
            &pre_pkt,
            crate::stream::TsStats::default(),
        );

        let mut initial = [0u8; 1024];
        let mut init_total = Vec::new();
        while !init_total.windows(8).any(|w| w == [0x11u8; 8]) {
            let n = client1.read(&mut initial).unwrap();
            assert!(n > 0, "unexpected early EOF before pre-restart chunk");
            init_total.extend_from_slice(&initial[..n]);
        }
        assert!(String::from_utf8_lossy(&init_total).contains("200 OK"));
        // Give handle_http_client a moment to re-enter rx.recv_timeout(200ms)
        thread::sleep(Duration::from_millis(20));

        // Drop srv1, reset cache, start srv2 on same port, and immediately publish post-restart chunk (0x22)
        // while the detached srv1 handler thread is still inside rx.recv_timeout().
        let drop_t0 = std::time::Instant::now();
        drop(srv1);
        hub.reset_cache();
        let srv2 = HttpServer::start(hub.clone(), "127.0.0.1", port).unwrap();
        let mut post_pkt = vec![0x22u8; 188];
        post_pkt[0] = 0x47;
        hub.publish_ts(
            post_pkt.clone(),
            true,
            &post_pkt,
            crate::stream::TsStats::default(),
        );

        // Drain client1 to EOF — it MUST NOT contain any post-restart 0x22 payload and MUST close quickly.
        let mut rest = Vec::new();
        let mut buf = [0u8; 1024];
        let mut saw_eof = false;
        loop {
            match client1.read(&mut buf) {
                Ok(0) => {
                    saw_eof = true;
                    break;
                }
                Ok(k) => rest.extend_from_slice(&buf[..k]),
                Err(_) => break,
            }
        }
        let elapsed = drop_t0.elapsed();
        drop(srv2);

        assert!(saw_eof, "Pre-restart HTTP client did not observe EOF");
        assert!(
            !rest.windows(8).any(|w| w == [0x22u8; 8]),
            "Pre-restart HTTP client leaked post-restart TS chunk after HttpServer::drop!"
        );
        assert!(
            elapsed < Duration::from_millis(120),
            "Pre-restart HTTP client took too long to receive EOF after drop: {:?}",
            elapsed
        );
    }

    #[test]
    fn srt_lan_client_url_never_exposes_unspecified_zero_address() {
        let hub = StreamHub::new();
        let srt = SrtOutput::start(&hub, "0.0.0.0", 14_861);
        assert!(
            !srt.url.contains("0.0.0.0"),
            "SRT client URL must not contain 0.0.0.0: {}",
            srt.url
        );
        assert!(srt.url.starts_with("srt://"));
        assert!(srt.url.contains(":14861?mode=caller"));
        drop(srt);
    }

    static LIVE_HOST_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    #[ignore = "Requires live Linux host with PipeWire (pactl/pw-cat) and v4l2loopback-ctl"]
    fn live_host_v4l2_mjpeg_zero_ffmpeg_and_pipewire_audio_work_end_to_end() {
        let _guard = LIVE_HOST_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        remove_v4l2_device("/dev/video14");
        // 1. Generate a real 640x360 JPEG frame and real H.264+AAC MPEG-TS via ffmpeg
        let jpg_out = Command::new("ffmpeg")
            .args([
                "-hide_banner", "-loglevel", "error", "-f", "lavfi",
                "-i", "testsrc=size=640x360:rate=1", "-frames:v", "1", "-f", "mjpeg", "pipe:1",
            ])
            .output()
            .expect("ffmpeg jpeg");
        assert!(jpg_out.status.success() && jpg_out.stdout.starts_with(&[0xFF, 0xD8]));

        // Test pure-Rust V4L2 MJPEG output (zero ffmpeg!) on /dev/video14
        let hub_mjpeg = StreamHub::new();
        let v4l2 = V4l2Output::start(
            &hub_mjpeg,
            CaptureMode::Mjpeg,
            "/dev/video14".to_string(),
            V4l2FormatPolicy::Auto,
        )
        .expect("V4l2Output Mjpeg start");
        for _ in 0..8 {
            hub_mjpeg.publish_jpeg(jpg_out.stdout.clone());
            thread::sleep(Duration::from_millis(30));
        }
        let probe = Command::new("ffprobe")
            .args([
                "-v", "error", "-f", "v4l2", "-i", "/dev/video14",
                "-show_entries", "stream=codec_name,width,height", "-of", "compact",
            ])
            .output()
            .expect("ffprobe v4l2");
        let probe_text = String::from_utf8_lossy(&probe.stdout);
        v4l2.stop();
        assert!(
            probe_text.contains("codec_name=mjpeg") && probe_text.contains("width=640") && probe_text.contains("height=360"),
            "Unexpected V4L2 MJPEG ffprobe output: {probe_text}"
        );
        assert!(!Path::new("/dev/video14").exists(), "/dev/video14 should be cleaned up on stop");

        // 2. Test PipeWireAudioOutput: exact "GL310 Audio" description, live VU > 0, and clean unload
        let ts_out = Command::new("ffmpeg")
            .args([
                "-hide_banner", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc=duration=2:size=640x360:rate=15",
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
                "-c:v", "libx264", "-preset", "ultrafast", "-g", "15",
                "-c:a", "aac", "-b:a", "128k", "-ac", "2",
                "-f", "mpegts", "pipe:1",
            ])
            .output()
            .expect("ffmpeg synth ts");
        assert!(ts_out.status.success() && ts_out.stdout.len() > 10_000);

        let hub_ts = StreamHub::new();
        let vu = VuMeter::new();
        let mon = MonitorControl::from_settings(&Settings::default());
        let pw = PipeWireAudioOutput::start(&hub_ts, vu.clone(), mon).expect("PipeWire start");

        let sources = Command::new("pactl").args(["list", "sources"]).output().unwrap();
        let sources_text = String::from_utf8_lossy(&sources.stdout);
        assert!(
            sources_text.contains("Description: GL310 Audio"),
            "Expected exact PipeWire source description 'GL310 Audio'"
        );

        // Publish synthetic TS and verify VU-meter picks up the 440 Hz tone
        let mut saw_audio = false;
        for chunk in ts_out.stdout.chunks(188 * 40) {
            hub_ts.publish_ts(chunk.to_vec(), true, &ts_out.stdout[..188 * 2], crate::stream::TsStats::default());
            thread::sleep(Duration::from_millis(25));
            let lv = vu.read();
            if lv.left > 0.05 && lv.right > 0.05 {
                saw_audio = true;
            }
        }
        thread::sleep(Duration::from_millis(150));
        let lv = vu.read();
        if lv.left > 0.05 && lv.right > 0.05 {
            saw_audio = true;
        }
        drop(pw);
        assert!(saw_audio, "Expected non-zero stereo VU-meter RMS from decoded 440 Hz AAC tone");

        // Confirm zero leftover gl310 PipeWire modules after drop
        let mods = Command::new("pactl").args(["list", "short", "modules"]).output().unwrap();
        let mods_text = String::from_utf8_lossy(&mods.stdout);
        assert!(!mods_text.contains("gl310_audio_sink"), "Leftover gl310_audio_sink module!");
        assert!(!mods_text.contains("gl310_audio_source"), "Leftover gl310_audio_source module!");
    }

    #[test]
    #[ignore = "Requires live Linux host with v4l2loopback-ctl and ffmpeg"]
    fn live_host_v4l2_restart_preserves_device_ownership_across_resolution_switch() {
        let _guard = LIVE_HOST_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dev = "/dev/video14".to_string();
        remove_v4l2_device(&dev);
        assert!(!Path::new(&dev).exists());

        // Generate two MPEG-TS streams at different resolutions (640x360 -> 1280x720)
        let make_ts = |size: &str| {
            let out = Command::new("ffmpeg")
                .args([
                    "-hide_banner", "-loglevel", "error",
                    "-f", "lavfi", "-i", &format!("testsrc=duration=1:size={size}:rate=15"),
                    "-c:v", "libx264", "-preset", "ultrafast", "-g", "15",
                    "-f", "mpegts", "pipe:1",
                ])
                .output()
                .expect("ffmpeg synth ts");
            assert!(out.status.success() && out.stdout.len() > 4096);
            out.stdout
        };
        let ts_low = make_ts("640x360");
        let ts_hi = make_ts("1280x720");

        let hub = StreamHub::new();
        let v1 = V4l2Output::start(&hub, CaptureMode::H264Aac, dev.clone(), V4l2FormatPolicy::Auto)
            .expect("initial V4l2Output start");
        for chunk in ts_low.chunks(188 * 32) {
            hub.publish_ts(chunk.to_vec(), true, &ts_low[..188 * 2], crate::stream::TsStats::default());
            thread::sleep(Duration::from_millis(15));
        }

        // Simulate OBS holding /dev/video14 open while an HDMI resolution change triggers restart
        let mut reader = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "v4l2", "-i", &dev, "-f", "null", "-"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn v4l2 reader");
        thread::sleep(Duration::from_millis(300));

        // Restart V4l2Output preserving ownership without deleting /dev/video14
        let owned = v1.stop_keep_device();
        assert_eq!(owned.as_deref(), Some(dev.as_str()));
        assert!(Path::new(&dev).exists(), "/dev/video14 must stay alive during restart");

        hub.reset_cache();
        let v2 = V4l2Output::start_with_owned(
            &hub,
            CaptureMode::H264Aac,
            dev.clone(),
            V4l2FormatPolicy::Auto,
            owned.is_some(),
        )
        .expect("restarted V4l2Output start");

        for chunk in ts_hi.chunks(188 * 32) {
            hub.publish_ts(chunk.to_vec(), true, &ts_hi[..188 * 2], crate::stream::TsStats::default());
            thread::sleep(Duration::from_millis(15));
        }

        // Close OBS reader first, then stop capture — /dev/video14 MUST be cleaned up because ownership was preserved!
        let _ = reader.kill();
        let _ = reader.wait();
        thread::sleep(Duration::from_millis(100));
        v2.stop();
        assert!(
            !Path::new(&dev).exists(),
            "/dev/video14 leaked after restart — created_device ownership was not preserved!"
        );
    }
}
