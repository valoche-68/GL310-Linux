//! `gl310-core`: Shared zero-GUI Rust hardware driver, stream hub, Linux output bridges,
//! and capture session orchestrator for the AVerMedia LGP Lite (GL310, USB `07ca:c835`).

pub mod device;
pub mod firmware;
pub mod outputs;
pub mod proto;
pub mod recorder;
pub mod settings;
pub mod stream;

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::device::{probe_usb, DeviceInfo, Gl310Device, UsbProbeInfo};
use crate::firmware::{discover_verified_firmware, inspect_firmware_status, FirmwareStatus};
use crate::outputs::{remove_v4l2_device, ActiveOutputs, LogSink, MonitorControl, VuLevels, VuMeter};
use crate::proto::{HdmiSignal, McuEvent};
use crate::recorder::{save_snapshot, RecordingStatus, RecordingSummary, TsRecorder};
use crate::settings::{
    default_config_path, AudioInput, ButtonAction, CaptureMode, LedMode, Settings, UiLanguage,
};
use crate::stream::{MjpegDeframer, StreamHub, TsDeframer, TsStats};

// ---------------------------------------------------------------------------
// Doctor / System Diagnostics
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCheck {
    pub name: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub usb: UsbProbeInfo,
    pub udev_rule_installed: bool,
    pub firmware: FirmwareStatus,
    pub tools: Vec<ToolCheck>,
    pub v4l2_devices: Vec<String>,
    pub ready_for_phase1: bool,
    pub ready_for_capture: bool,
}

fn check_cmd(bin: &str, args: &[&str]) -> ToolCheck {
    match Command::new(bin).args(args).output() {
        Ok(out) => {
            let first_line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let detail = if !first_line.is_empty() {
                first_line
            } else {
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("installed")
                    .trim()
                    .to_string()
            };
            ToolCheck {
                name: bin.to_string(),
                available: out.status.success(),
                detail,
            }
        }
        Err(_) => ToolCheck {
            name: bin.to_string(),
            available: false,
            detail: "not found in PATH".to_string(),
        },
    }
}

/// Run full host environment & hardware diagnostic without requiring root privileges.
pub fn run_doctor(firmware_override: Option<&Path>) -> DoctorReport {
    let usb = probe_usb();
    let udev_rule_installed = Path::new("/etc/udev/rules.d/70-avermedia-gl310.rules").is_file();
    let firmware = inspect_firmware_status(firmware_override);

    let tools = vec![
        check_cmd("ffmpeg", &["-version"]),
        check_cmd("v4l2loopback-ctl", &["-v"]),
        check_cmd("pactl", &["--version"]),
        check_cmd("pw-cat", &["--version"]),
        check_cmd("pw-link", &["--version"]),
    ];

    let v4l2_devices = Command::new("v4l2loopback-ctl")
        .arg("list")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let ready_for_phase1 = usb.present && usb.can_open && firmware.valid;
    let ffmpeg_ok = tools.iter().any(|t| t.name == "ffmpeg" && t.available);
    let ready_for_capture = ready_for_phase1 && ffmpeg_ok;

    DoctorReport {
        usb,
        udev_rule_installed,
        firmware,
        tools,
        v4l2_devices,
        ready_for_phase1,
        ready_for_capture,
    }
}

// ---------------------------------------------------------------------------
// LAN Display Host Detection (Zero-Packet UDP Route Lookup)
// ---------------------------------------------------------------------------

/// Detect the host's primary local LAN IPv4 address without sending any network packets.
/// Uses `UdpSocket::bind("0.0.0.0:0")` + `.connect("1.1.1.1:80")` + `.local_addr()`.
/// Returns `None` if offline or if only an unspecified (`0.0.0.0`) or loopback address is found.
pub fn detect_lan_ipv4() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_loopback() => Some(v4.to_string()),
        _ => None,
    }
}

/// Resolve the copyable host address for SRT/HTTP/MJPEG URLs.
/// Always returns `"127.0.0.1"` when `expose_lan` is `false`, and returns the detected
/// primary LAN IPv4 address (falling back to `"127.0.0.1"` if offline) when `expose_lan` is `true`.
pub fn display_host_for(expose_lan: bool) -> String {
    if expose_lan {
        detect_lan_ipv4().unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        "127.0.0.1".to_string()
    }
}

// ---------------------------------------------------------------------------
// Unified Capture Engine (Shared by Tauri v2 Studio & CLI)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePhase {
    Absent,
    PermissionDenied,
    LockedD835,
    FirmwareMissing,
    Booting,
    Idle,
    Streaming,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub phase: EnginePhase,
    pub status_message: String,
    pub device: Option<DeviceInfo>,
    pub signal: Option<HdmiSignal>,
    pub out_width: u32,
    pub out_height: u32,
    pub out_fps: u32,
    pub bitrate_mbps: f32,
    pub ts_stats: TsStats,
    pub mjpeg_frames: u64,
    pub vu: VuLevels,
    pub recording: RecordingStatus,
    pub firmware: FirmwareStatus,
    pub settings: Settings,
    pub stream_generation: u64,
    pub display_host: String,
    pub http_url: Option<String>,
    pub mjpeg_url: Option<String>,
    pub snapshot_url: Option<String>,
    pub srt_url: Option<String>,
    pub v4l2_active: bool,
    pub pipewire_active: bool,
    pub preview_active: bool,
    pub hdmi_out_passthrough: String,
}

struct EngineState {
    phase: EnginePhase,
    status_message: String,
    device_info: Option<DeviceInfo>,
    signal: Option<HdmiSignal>,
    out_width: u32,
    out_height: u32,
    out_fps: u32,
    bitrate_mbps: f32,
    settings: Settings,
    firmware: FirmwareStatus,
    stream_generation: u64,
    logs: VecDeque<LogEntry>,
    recorder: Option<TsRecorder>,
    display_host: String,
    http_url: Option<String>,
    mjpeg_url: Option<String>,
    snapshot_url: Option<String>,
    srt_url: Option<String>,
    v4l2_active: bool,
    pipewire_active: bool,
    preview_active: bool,
}

type SnapshotReply = std::sync::mpsc::SyncSender<Result<EngineSnapshot, String>>;

enum EngineCmd {
    StartCapture(Option<SnapshotReply>),
    StopCapture(Option<SnapshotReply>),
    ApplySettings(Settings),
    SetPreviewEnabled(bool, Option<SnapshotReply>),
    SetWindowVisible(bool),
    SetLed(LedMode),
    SetAudioInput(AudioInput),
    SetAnalogVolume(u8),
    SetEncoderVolume(u8),
    RefreshProbe,
    Shutdown,
}

/// Returns true only when changing settings requires restarting the hardware encoder or output sinks.
/// Updating live preferences (`button_action`, `led_mode`, `pc_monitor*`, `recordings_dir`) must NOT
/// abort an active recording or drop OBS streams.
fn settings_require_restart(old: &Settings, new: &Settings) -> bool {
    old.mode != new.mode
        || old.fps_policy != new.fps_policy
        || old.video_kbps != new.video_kbps
        || old.gop != new.gop
        || old.mjpeg_resolution != new.mjpeg_resolution
        || old.mjpeg_fps != new.mjpeg_fps
        || old.mjpeg_quality != new.mjpeg_quality
        || old.audio_input != new.audio_input
        || old.audio_kbps != new.audio_kbps
        || old.audio_out_jack != new.audio_out_jack
        || old.enable_v4l2 != new.enable_v4l2
        || old.v4l2_device != new.v4l2_device
        || old.v4l2_format != new.v4l2_format
        || old.enable_pipewire_audio != new.enable_pipewire_audio
        || old.enable_srt != new.enable_srt
        || old.srt_port != new.srt_port
        || old.enable_http != new.enable_http
        || old.http_port != new.http_port
        || old.expose_lan != new.expose_lan
}

/// High-level background orchestrator owning the USB handle, StreamHub, Linux outputs,
/// physical button handler, LED ring, and local `.ts -> .mp4` recorder.
pub struct CaptureEngine {
    state: Arc<Mutex<EngineState>>,
    hub: StreamHub,
    vu: VuMeter,
    monitor: MonitorControl,
    cmd_tx: std::sync::mpsc::Sender<EngineCmd>,
    stop_flag: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    config_path: PathBuf,
}

impl CaptureEngine {
    pub fn start(initial_settings: Option<Settings>) -> Self {
        let config_path = default_config_path();
        let settings = initial_settings.unwrap_or_else(|| Settings::load_or_default(&config_path));
        let initial_fw = inspect_firmware_status(settings.firmware_dir_override.as_deref().map(Path::new));
        let initial_display_host = display_host_for(settings.expose_lan);
        let monitor = MonitorControl::from_settings(&settings);
        let vu = VuMeter::new();
        let hub = StreamHub::new();

        let state = Arc::new(Mutex::new(EngineState {
            phase: EnginePhase::Absent,
            status_message: "Scanning USB bus for AVerMedia GL310 (07ca:c835)...".into(),
            device_info: None,
            signal: None,
            out_width: 0,
            out_height: 0,
            out_fps: 0,
            bitrate_mbps: 0.0,
            settings,
            firmware: initial_fw,
            stream_generation: 0,
            logs: VecDeque::with_capacity(300),
            recorder: None,
            display_host: initial_display_host,
            http_url: None,
            mjpeg_url: None,
            snapshot_url: None,
            srt_url: None,
            v4l2_active: false,
            pipewire_active: false,
            preview_active: false,
        }));

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let stop_flag = Arc::new(AtomicBool::new(false));

        let state_bg = Arc::clone(&state);
        let hub_bg = hub.clone();
        let vu_bg = vu.clone();
        let mon_bg = monitor.clone();
        let stop_bg = Arc::clone(&stop_flag);

        let worker = thread::spawn(move || {
            engine_worker_loop(state_bg, hub_bg, vu_bg, mon_bg, cmd_rx, stop_bg);
        });

        Self {
            state,
            hub,
            vu,
            monitor,
            cmd_tx,
            stop_flag,
            worker: Mutex::new(Some(worker)),
            config_path,
        }
    }

    pub fn hub(&self) -> StreamHub {
        self.hub.clone()
    }

    /// Re-scan firmware directories and update the cached `FirmwareStatus` in `EngineState`.
    pub fn refresh_firmware_status(&self) -> FirmwareStatus {
        let fw_ov = self
            .state
            .lock()
            .unwrap()
            .settings
            .firmware_dir_override
            .clone();
        let status = inspect_firmware_status(fw_ov.as_deref().map(Path::new));
        self.state.lock().unwrap().firmware = status.clone();
        status
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        build_snapshot(&self.state, &self.hub, &self.vu)
    }

    pub fn logs(&self) -> Vec<LogEntry> {
        self.state.lock().unwrap().logs.iter().cloned().collect()
    }

    pub fn clear_logs(&self) {
        self.state.lock().unwrap().logs.clear();
    }

    pub fn push_log(&self, level: &str, msg: impl Into<String>) {
        push_state_log(&self.state, level, msg);
    }

    pub fn start_capture(&self) -> Result<(), String> {
        self.cmd_tx
            .send(EngineCmd::StartCapture(None))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    /// Start capture and wait for the worker thread to complete hardware encoder & output startup,
    /// returning the resulting `EngineSnapshot` or the exact error message on failure.
    pub fn start_capture_sync(&self) -> Result<EngineSnapshot, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(EngineCmd::StartCapture(Some(tx)))
            .map_err(|e| format!("Engine channel closed: {e}"))?;
        rx.recv_timeout(Duration::from_secs(12))
            .map_err(|_| "Timeout while starting GL310 capture engine".to_string())?
    }

    pub fn stop_capture(&self) -> Result<(), String> {
        self.cmd_tx
            .send(EngineCmd::StopCapture(None))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    /// Stop capture and wait for the worker thread to tear down outputs and stop the hardware encoder.
    pub fn stop_capture_sync(&self) -> Result<EngineSnapshot, String> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(EngineCmd::StopCapture(Some(tx)))
            .map_err(|e| format!("Engine channel closed: {e}"))?;
        rx.recv_timeout(Duration::from_secs(12))
            .map_err(|_| "Timeout while stopping GL310 capture engine".to_string())?
    }

    pub fn update_settings(&self, new_settings: Settings) -> Result<Settings, String> {
        new_settings.validate()?;
        let _ = new_settings.save(&self.config_path);
        self.monitor.update(
            new_settings.pc_monitor,
            new_settings.pc_monitor_muted,
            new_settings.pc_monitor_volume,
        );
        self.cmd_tx
            .send(EngineCmd::ApplySettings(new_settings.clone()))
            .map_err(|e| format!("Engine channel closed: {e}"))?;
        Ok(new_settings)
    }

    pub fn set_led(&self, mode: LedMode) -> Result<(), String> {
        {
            let mut g = self.state.lock().unwrap();
            g.settings.led_mode = mode;
            let _ = g.settings.save(&self.config_path);
        }
        self.cmd_tx
            .send(EngineCmd::SetLed(mode))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    pub fn set_audio_input(&self, input: AudioInput) -> Result<(), String> {
        {
            let mut g = self.state.lock().unwrap();
            g.settings.audio_input = input;
            let _ = g.settings.save(&self.config_path);
        }
        self.cmd_tx
            .send(EngineCmd::SetAudioInput(input))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    pub fn set_analog_volume(&self, vol: u8) -> Result<(), String> {
        let clamped = vol.clamp(1, 11);
        {
            let mut g = self.state.lock().unwrap();
            g.settings.analog_volume = clamped;
            let _ = g.settings.save(&self.config_path);
        }
        self.cmd_tx
            .send(EngineCmd::SetAnalogVolume(clamped))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    pub fn set_encoder_volume(&self, vol: u8) -> Result<(), String> {
        let clamped = vol.min(15);
        {
            let mut g = self.state.lock().unwrap();
            g.settings.encoder_volume = clamped;
            let _ = g.settings.save(&self.config_path);
        }
        self.cmd_tx
            .send(EngineCmd::SetEncoderVolume(clamped))
            .map_err(|e| format!("Engine channel closed: {e}"))
    }

    pub fn set_pc_monitor(&self, enabled: bool, muted: bool, volume: u8) {
        let clamped = volume.min(100);
        self.monitor.update(enabled, muted, clamped);
        let mut g = self.state.lock().unwrap();
        g.settings.pc_monitor = enabled;
        g.settings.pc_monitor_muted = muted;
        g.settings.pc_monitor_volume = clamped;
        let _ = g.settings.save(&self.config_path);
    }

    /// Live-toggle the Studio video preview (dynamically stops/starts `PreviewBridge` in H.264 mode
    /// without restarting the hardware encoder or disturbing V4L2/PipeWire/SRT/REC).
    pub fn set_preview_enabled(&self, enabled: bool) -> Result<EngineSnapshot, String> {
        {
            let mut g = self.state.lock().unwrap();
            g.settings.enable_preview = enabled;
            let _ = g.settings.save(&self.config_path);
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(EngineCmd::SetPreviewEnabled(enabled, Some(tx)))
            .map_err(|e| format!("Engine channel closed: {e}"))?;
        rx.recv_timeout(Duration::from_secs(6))
            .map_err(|_| "Timeout while updating preview state".to_string())?
    }

    /// Notify the engine when the Studio window is shown or hidden/minimized to the system tray so
    /// `PreviewBridge` pauses automatically when hidden (unless an external `/mjpeg` viewer is connected).
    pub fn set_window_visible(&self, visible: bool) {
        let _ = self.cmd_tx.send(EngineCmd::SetWindowVisible(visible));
    }

    /// Persist the Studio UI language (`en` or `fr`) without disturbing capture or recording.
    pub fn set_ui_language(&self, lang: UiLanguage) -> Result<EngineSnapshot, String> {
        {
            let mut g = self.state.lock().unwrap();
            g.settings.ui_language = lang;
            let _ = g.settings.save(&self.config_path);
        }
        Ok(self.snapshot())
    }

    pub fn start_recording(&self, path_override: Option<PathBuf>) -> Result<RecordingStatus, String> {
        let mut g = self.state.lock().unwrap();
        if g.phase != EnginePhase::Streaming {
            return Err("Start capture before recording".into());
        }
        if g.settings.mode != CaptureMode::H264Aac {
            return Err("MP4/TS recording requires Studio OBS (H.264 + AAC) mode".into());
        }
        if g.recorder.is_some() {
            return Err("Recording is already active".into());
        }
        let target = path_override
            .unwrap_or_else(|| TsRecorder::default_mp4_path(Path::new(&g.settings.recordings_dir)));
        let rec = TsRecorder::start(&self.hub, target)?;
        let st = rec.status();
        g.recorder = Some(rec);
        drop(g);
        push_state_log(
            &self.state,
            "info",
            format!("Recording started -> {}", st.path.as_deref().unwrap_or("")),
        );
        Ok(st)
    }

    pub fn stop_recording(&self) -> Result<RecordingSummary, String> {
        let rec = {
            let mut g = self.state.lock().unwrap();
            g.recorder
                .take()
                .ok_or_else(|| "No active recording".to_string())?
        };
        let summary = rec.stop();
        push_state_log(
            &self.state,
            "info",
            format!(
                "Recording stopped -> {} ({:.1} s, {} bytes — {})",
                summary.final_path, summary.duration_secs, summary.bytes, summary.note
            ),
        );
        Ok(summary)
    }

    pub fn take_snapshot(&self) -> Result<String, String> {
        let (rec_dir, mode, enable_preview) = {
            let g = self.state.lock().unwrap();
            (
                g.settings.recordings_dir.clone(),
                g.settings.mode,
                g.settings.enable_preview,
            )
        };
        if mode == CaptureMode::H264Aac && !enable_preview && self.hub.latest_jpeg().is_none() {
            return Err(
                "Preview disabled — re-enable preview to take a JPEG snapshot in H.264 mode"
                    .to_string(),
            );
        }
        let path = save_snapshot(&self.hub, Path::new(&rec_dir))?;
        let s = path.display().to_string();
        push_state_log(&self.state, "info", format!("Snapshot saved -> {s}"));
        Ok(s)
    }

    pub fn refresh_probe(&self) {
        let _ = self.refresh_firmware_status();
        let _ = self.cmd_tx.send(EngineCmd::RefreshProbe);
    }

    /// Deterministically stop hardware capture, finalize any active `.mp4` recording,
    /// tear down V4L2/PipeWire/SRT/HTTP outputs, and join the worker thread. Idempotent.
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(EngineCmd::Shutdown);
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.worker.lock().unwrap().take() {
            let _ = h.join();
        }
    }
}

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build_snapshot(state: &Arc<Mutex<EngineState>>, hub: &StreamHub, vu: &VuMeter) -> EngineSnapshot {
    let g = state.lock().unwrap();
    let recording = g
        .recorder
        .as_ref()
        .map(TsRecorder::status)
        .unwrap_or(RecordingStatus {
            active: false,
            path: None,
            bytes: 0,
            duration_secs: 0.0,
        });
    EngineSnapshot {
        phase: g.phase,
        status_message: g.status_message.clone(),
        device: g.device_info.clone(),
        signal: g.signal,
        out_width: g.out_width,
        out_height: g.out_height,
        out_fps: g.out_fps,
        bitrate_mbps: g.bitrate_mbps,
        ts_stats: hub.ts_stats(),
        mjpeg_frames: hub.mjpeg_frames(),
        vu: vu.read(),
        recording,
        firmware: g.firmware.clone(),
        settings: g.settings.clone(),
        stream_generation: g.stream_generation,
        display_host: g.display_host.clone(),
        http_url: g.http_url.clone(),
        mjpeg_url: g.mjpeg_url.clone(),
        snapshot_url: g.snapshot_url.clone(),
        srt_url: g.srt_url.clone(),
        v4l2_active: g.v4l2_active,
        pipewire_active: g.pipewire_active,
        preview_active: g.preview_active,
        hdmi_out_passthrough: "Hardware HDMI loop-through (always active when HDMI IN is locked)".into(),
    }
}

/// Persistent diagnostic log path (`~/.local/state/gl310-linux/studio.log`).
pub fn default_log_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("gl310-linux").join("studio.log"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(
                PathBuf::from(home)
                    .join(".local")
                    .join("state")
                    .join("gl310-linux")
                    .join("studio.log"),
            );
        }
    }
    None
}

fn append_persistent_log(level: &str, msg: &str) {
    let Some(path) = default_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Truncate if log ever grows past 2 MiB
    if fs::metadata(&path).map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
        let _ = fs::write(&path, "");
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let stamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{stamp}] [{}] {msg}", level.to_ascii_uppercase());
    }
}

fn push_state_log(state: &Arc<Mutex<EngineState>>, level: &str, msg: impl Into<String>) {
    let text = msg.into();
    append_persistent_log(level, &text);
    let mut g = state.lock().unwrap();
    if g.logs.len() >= 300 {
        g.logs.pop_front();
    }
    g.logs.push_back(LogEntry {
        ts: Local::now().format("%H:%M:%S").to_string(),
        level: level.to_string(),
        message: text,
    });
}

fn engine_worker_loop(
    state: Arc<Mutex<EngineState>>,
    hub: StreamHub,
    vu: VuMeter,
    monitor: MonitorControl,
    cmd_rx: std::sync::mpsc::Receiver<EngineCmd>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut device: Option<Gl310Device> = None;
    let mut active_outputs: Option<ActiveOutputs> = None;
    let mut streaming = false;
    let mut window_visible = true;
    let mut ts_deframer = TsDeframer::new();
    let mut mjpeg_deframer = MjpegDeframer::new();

    let mut last_probe = Instant::now() - Duration::from_secs(10);
    let mut last_signal_poll = Instant::now() - Duration::from_secs(10);
    let mut last_rate_tick = Instant::now();
    let mut bytes_since_tick: u64 = 0;
    let mut stream_started_at: Option<Instant> = None;
    let mut no_data_warned = false;

    while !stop_flag.load(Ordering::SeqCst) {
        // 1. Drain commands non-blockingly
        let mut should_exit = false;
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                EngineCmd::Shutdown => {
                    should_exit = true;
                    break;
                }
                EngineCmd::RefreshProbe => {
                    last_probe = Instant::now() - Duration::from_secs(10);
                    let cfg = state.lock().unwrap().settings.clone();
                    let host = display_host_for(cfg.expose_lan);
                    let mut g = state.lock().unwrap();
                    if let Some(ref outputs) = active_outputs {
                        apply_active_outputs_to_state(&mut g, &cfg, outputs, host);
                    } else {
                        g.display_host = host;
                    }
                }
                EngineCmd::SetPreviewEnabled(enabled, reply) => {
                    {
                        let mut g = state.lock().unwrap();
                        g.settings.enable_preview = enabled;
                    }
                    if streaming {
                        sync_preview_bridge(&state, &hub, &mut active_outputs, window_visible);
                    }
                    if let Some(tx) = reply {
                        let _ = tx.send(Ok(build_snapshot(&state, &hub, &vu)));
                    }
                }
                EngineCmd::SetWindowVisible(visible) => {
                    if window_visible != visible {
                        window_visible = visible;
                        if streaming {
                            sync_preview_bridge(&state, &hub, &mut active_outputs, window_visible);
                        }
                    }
                }
                EngineCmd::SetLed(mode) => {
                    if let Some(ref dev) = device {
                        if let Err(e) = dev.set_led_mode(mode) {
                            push_state_log(&state, "warn", format!("LED update failed: {e}"));
                        } else {
                            push_state_log(&state, "info", format!("LED ring set to {mode:?}"));
                        }
                    }
                }
                EngineCmd::SetAudioInput(input) => {
                    if let Some(ref dev) = device {
                        if let Err(e) = dev.set_audio_input(input) {
                            push_state_log(&state, "warn", format!("Audio input switch failed: {e}"));
                        } else {
                            push_state_log(
                                &state,
                                "info",
                                format!("Hardware audio input switched to {input:?}"),
                            );
                        }
                    }
                }
                EngineCmd::SetAnalogVolume(vol) => {
                    if let Some(ref dev) = device {
                        if let Err(e) = dev.set_analog_input_volume(vol) {
                            push_state_log(&state, "warn", format!("Analog gain failed: {e}"));
                        } else {
                            push_state_log(&state, "info", format!("Jack AUDIO IN gain set to {vol}/11"));
                        }
                    }
                }
                EngineCmd::SetEncoderVolume(vol) => {
                    if let Some(ref dev) = device {
                        if let Err(e) = dev.set_encoder_audio_volume(vol) {
                            push_state_log(&state, "warn", format!("Encoder audio volume failed: {e}"));
                        } else {
                            push_state_log(&state, "info", format!("Encoder digital volume set to {vol}/15"));
                        }
                    }
                }
                EngineCmd::ApplySettings(new_cfg) => {
                    let next_display_host = display_host_for(new_cfg.expose_lan);
                    let (old_cfg, fw_dir_changed) = {
                        let mut g = state.lock().unwrap();
                        let old = g.settings.clone();
                        let fw_changed = old.firmware_dir_override != new_cfg.firmware_dir_override;
                        g.settings = new_cfg.clone();
                        g.display_host = next_display_host.clone();
                        (old, fw_changed)
                    };
                    if fw_dir_changed {
                        let st = inspect_firmware_status(
                            new_cfg.firmware_dir_override.as_deref().map(Path::new),
                        );
                        state.lock().unwrap().firmware = st;
                    }
                    let must_restart = settings_require_restart(&old_cfg, &new_cfg);
                    if streaming && must_restart {
                        push_state_log(&state, "info", "Applying settings — restarting encoder cleanly");
                        stop_recording_if_active(&state);
                        let preserved_v4l2 = active_outputs
                            .take()
                            .and_then(ActiveOutputs::take_owned_v4l2_device);
                        hub.reset_cache();
                        if let Some(ref dev) = device {
                            let _ = dev.stop_encoder();
                        }
                        streaming = false;
                        bytes_since_tick = 0;
                        last_rate_tick = Instant::now();
                        // Re-trigger capture right away
                        if let Some(ref dev) = device {
                            start_capture_internal(
                                dev,
                                &state,
                                &hub,
                                &vu,
                                &monitor,
                                &mut active_outputs,
                                &mut streaming,
                                &mut ts_deframer,
                                &mut mjpeg_deframer,
                                preserved_v4l2,
                                window_visible,
                            );
                        } else if let Some(dev_path) = preserved_v4l2 {
                            remove_v4l2_device(&dev_path);
                        }
                    } else if streaming {
                        sync_preview_bridge(&state, &hub, &mut active_outputs, window_visible);
                        if let Some(ref outputs) = active_outputs {
                            let mut g = state.lock().unwrap();
                            apply_active_outputs_to_state(&mut g, &new_cfg, outputs, next_display_host);
                        }
                    } else if let Some(ref outputs) = active_outputs {
                        let mut g = state.lock().unwrap();
                        apply_active_outputs_to_state(&mut g, &new_cfg, outputs, next_display_host);
                    }
                }
                EngineCmd::StartCapture(reply) => {
                    let result = if streaming {
                        Ok(build_snapshot(&state, &hub, &vu))
                    } else if let Some(ref dev) = device {
                        bytes_since_tick = 0;
                        last_rate_tick = Instant::now();
                        start_capture_internal(
                            dev,
                            &state,
                            &hub,
                            &vu,
                            &monitor,
                            &mut active_outputs,
                            &mut streaming,
                            &mut ts_deframer,
                            &mut mjpeg_deframer,
                            None,
                            window_visible,
                        );
                        if streaming {
                            stream_started_at = Some(Instant::now());
                            no_data_warned = false;
                            Ok(build_snapshot(&state, &hub, &vu))
                        } else {
                            stream_started_at = None;
                            let msg = state.lock().unwrap().status_message.clone();
                            Err(msg)
                        }
                    } else {
                        let msg = state.lock().unwrap().status_message.clone();
                        push_state_log(
                            &state,
                            "warn",
                            format!("Cannot start capture: GL310 device not ready ({msg})"),
                        );
                        Err(format!("GL310 device not ready: {msg}"))
                    };
                    if let Some(tx) = reply {
                        let _ = tx.send(result);
                    }
                }
                EngineCmd::StopCapture(reply) => {
                    if streaming {
                        stop_recording_if_active(&state);
                        active_outputs = None;
                        hub.reset_cache();
                        if let Some(ref dev) = device {
                            let _ = dev.stop_encoder();
                        }
                        streaming = false;
                        bytes_since_tick = 0;
                        last_rate_tick = Instant::now();
                        let mut g = state.lock().unwrap();
                        g.phase = EnginePhase::Idle;
                        g.status_message = "Ready (encoder stopped)".into();
                        clear_streaming_state(&mut g);
                        drop(g);
                        push_state_log(&state, "info", "Capture stopped cleanly");
                    }
                    if let Some(tx) = reply {
                        let _ = tx.send(Ok(build_snapshot(&state, &hub, &vu)));
                    }
                }
            }
        }

        if should_exit {
            break;
        }

        // 2. Device discovery & automatic RAM firmware boot
        if device.is_none() {
            if last_probe.elapsed() >= Duration::from_secs(2) {
                last_probe = Instant::now();
                let probe = probe_usb();
                if !probe.present {
                    let mut g = state.lock().unwrap();
                    g.phase = EnginePhase::Absent;
                    g.status_message = "No AVerMedia GL310 (07ca:c835) detected on USB".into();
                    g.device_info = None;
                    g.signal = None;
                } else if probe.locked_d835 {
                    let mut g = state.lock().unwrap();
                    g.phase = EnginePhase::LockedD835;
                    g.status_message =
                        "GL310 in locked 07ca:d835 state — unplug 10 seconds and reconnect".into();
                } else if !probe.can_open {
                    let mut g = state.lock().unwrap();
                    g.phase = EnginePhase::PermissionDenied;
                    g.status_message = probe
                        .open_error
                        .unwrap_or_else(|| "USB permission denied (install udev rule)".into());
                } else {
                    match Gl310Device::open() {
                        Ok(dev) => {
                            push_state_log(
                                &state,
                                "info",
                                format!(
                                    "Opened GL310 on USB bus {:03} dev {:03}",
                                    dev.bus(),
                                    dev.address()
                                ),
                            );
                            let loaded = dev.firmware_loaded().unwrap_or(false)
                                && dev.ping_mailbox().is_ok();
                            if !loaded {
                                push_state_log(
                                    &state,
                                    "info",
                                    "ARM firmware absent or unresponsive after sleep/reset — booting verified firmware into RAM",
                                );
                                let fw_ov = state
                                    .lock()
                                    .unwrap()
                                    .settings
                                    .firmware_dir_override
                                    .clone();
                                match discover_verified_firmware(fw_ov.as_deref().map(Path::new)) {
                                    Ok(fw) => {
                                        {
                                            let mut g = state.lock().unwrap();
                                            g.phase = EnginePhase::Booting;
                                            g.status_message =
                                                "Booting verified firmware into ARM RAM...".into();
                                        }
                                        let boot_res = dev.boot_firmware(&fw, |step| {
                                            push_state_log(&state, "info", format!("Boot: {step}"));
                                            let mut g = state.lock().unwrap();
                                            g.status_message = step.to_string();
                                        });
                                        if let Err(e) = boot_res {
                                            push_state_log(&state, "error", format!("RAM boot failed: {e}"));
                                            let mut g = state.lock().unwrap();
                                            g.phase = EnginePhase::Error;
                                            g.status_message = format!("Boot failed: {e}");
                                            continue;
                                        }
                                    }
                                    Err(e) => {
                                        let mut g = state.lock().unwrap();
                                        g.phase = EnginePhase::FirmwareMissing;
                                        g.status_message = e;
                                        continue;
                                    }
                                }
                            }
                            let info = dev.read_info().ok();
                            let sig = info.as_ref().and_then(|i| i.signal);
                            {
                                let mut g = state.lock().unwrap();
                                g.phase = EnginePhase::Idle;
                                g.status_message = "GL310 ready (firmware active in RAM)".into();
                                g.device_info = info;
                                g.signal = sig;
                            }
                            device = Some(dev);
                        }
                        Err(e) => {
                            let mut g = state.lock().unwrap();
                            g.phase = EnginePhase::Error;
                            g.status_message = e.to_string();
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        // 3. Streaming frame pump
        if streaming {
            let mode = state.lock().unwrap().settings.mode;
            let poll_res = device.as_ref().unwrap().poll_stream_chunk(mode);
            match poll_res {
                Ok(Some(chunk)) => {
                    bytes_since_tick += chunk.data.len() as u64;
                    match mode {
                        CaptureMode::H264Aac => {
                            let (aligned, idr) = ts_deframer.push(&chunk.data);
                            let pat_pmt = ts_deframer.bootstrap_headers();
                            hub.publish_ts(aligned, idr, &pat_pmt, ts_deframer.stats());
                        }
                        CaptureMode::Mjpeg => {
                            for frame in mjpeg_deframer.push(&chunk.data) {
                                hub.publish_jpeg(frame);
                            }
                        }
                    }
                }
                Ok(None) => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => {
                    push_state_log(&state, "error", format!("USB stream read error: {e}"));
                    stop_recording_if_active(&state);
                    active_outputs = None;
                    hub.reset_cache();
                    streaming = false;
                    bytes_since_tick = 0;
                    last_rate_tick = Instant::now();
                    device = None;
                    let mut g = state.lock().unwrap();
                    clear_streaming_state(&mut g);
                    g.phase = EnginePhase::Error;
                    g.status_message = format!("Disconnected: {e}");
                    continue;
                }
            }

            if last_rate_tick.elapsed() >= Duration::from_secs(1) {
                let secs = last_rate_tick.elapsed().as_secs_f32().max(0.1);
                let mbps = (bytes_since_tick as f32 * 8.0) / (secs * 1_000_000.0);
                bytes_since_tick = 0;
                last_rate_tick = Instant::now();
                state.lock().unwrap().bitrate_mbps = mbps;

                if !no_data_warned {
                    if let Some(started) = stream_started_at {
                        if started.elapsed() >= Duration::from_secs(4) {
                            let ts_pkts = hub.ts_stats().packets;
                            let mj_frames = hub.mjpeg_frames();
                            let preview_expected = state.lock().unwrap().preview_active;
                            if (mode == CaptureMode::H264Aac
                                && (ts_pkts == 0 || (preview_expected && mj_frames == 0)))
                                || (mode == CaptureMode::Mjpeg && mj_frames == 0)
                            {
                                no_data_warned = true;
                                let sig_desc = {
                                    let g = state.lock().unwrap();
                                    match g.signal {
                                        Some(s) => format!(
                                            "{}x{}@{}Hz HDCP={} audio={}({}Hz)",
                                            s.width,
                                            s.height,
                                            s.refresh_hz,
                                            s.hdcp,
                                            s.audio.present,
                                            s.audio.sample_rate_hz
                                        ),
                                        None => "NO HDMI SIGNAL".to_string(),
                                    }
                                };
                                push_state_log(
                                    &state,
                                    "warn",
                                    format!(
                                        "No preview frames after 4s (mode={mode:?}, ts_packets={ts_pkts}, mjpeg_frames={mj_frames}, signal={sig_desc})"
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            thread::sleep(Duration::from_millis(40));
        }

        // 4. Periodic signal & MCU button event polling (every 500 ms idle, 750 ms streaming)
        let poll_interval = if streaming {
            Duration::from_millis(750)
        } else {
            Duration::from_millis(500)
        };
        if last_signal_poll.elapsed() >= poll_interval {
            last_signal_poll = Instant::now();
            if streaming {
                // Dynamically wake/pause PreviewBridge if an external /mjpeg viewer connects/disconnects
                // while Studio is minimized to the system tray.
                sync_preview_bridge(&state, &hub, &mut active_outputs, window_visible);
            }
            if let Some(ref dev) = device {
                // Check physical button / MCU events
                if let Ok(ev) = dev.poll_mcu_event(!streaming) {
                    match ev {
                        McuEvent::ButtonClick => {
                            let action = state.lock().unwrap().settings.button_action;
                            push_state_log(
                                &state,
                                "info",
                                format!("Physical button clicked -> action {action:?}"),
                            );
                            match action {
                                ButtonAction::ToggleCapture => {
                                    if streaming {
                                        stop_recording_if_active(&state);
                                        active_outputs = None;
                                        hub.reset_cache();
                                        let _ = dev.stop_encoder();
                                        streaming = false;
                                        bytes_since_tick = 0;
                                        last_rate_tick = Instant::now();
                                        let mut g = state.lock().unwrap();
                                        clear_streaming_state(&mut g);
                                        g.phase = EnginePhase::Idle;
                                        g.status_message = "Stopped by hardware button".into();
                                    } else {
                                        bytes_since_tick = 0;
                                        last_rate_tick = Instant::now();
                                        start_capture_internal(
                                            dev,
                                            &state,
                                            &hub,
                                            &vu,
                                            &monitor,
                                            &mut active_outputs,
                                            &mut streaming,
                                            &mut ts_deframer,
                                            &mut mjpeg_deframer,
                                            None,
                                            window_visible,
                                        );
                                    }
                                }
                                ButtonAction::ToggleRecord => {
                                    let (rec_active, mode, rec_dir) = {
                                        let g = state.lock().unwrap();
                                        (
                                            g.recorder.is_some(),
                                            g.settings.mode,
                                            g.settings.recordings_dir.clone(),
                                        )
                                    };
                                    if rec_active {
                                        stop_recording_if_active(&state);
                                    } else if streaming {
                                        if mode != CaptureMode::H264Aac {
                                            push_state_log(
                                                &state,
                                                "warn",
                                                "Button REC ignored: .mp4 recording requires Studio OBS (H.264+AAC) mode",
                                            );
                                        } else {
                                            let target = TsRecorder::default_mp4_path(Path::new(&rec_dir));
                                            if let Ok(rec) = TsRecorder::start(&hub, target) {
                                                state.lock().unwrap().recorder = Some(rec);
                                                push_state_log(&state, "info", "Recording toggled ON by button");
                                            }
                                        }
                                    }
                                }
                                ButtonAction::Snapshot => {
                                    let (rec_dir, mode, enable_preview) = {
                                        let g = state.lock().unwrap();
                                        (
                                            g.settings.recordings_dir.clone(),
                                            g.settings.mode,
                                            g.settings.enable_preview,
                                        )
                                    };
                                    if mode == CaptureMode::H264Aac
                                        && !enable_preview
                                        && hub.latest_jpeg().is_none()
                                    {
                                        push_state_log(
                                            &state,
                                            "warn",
                                            "Preview disabled — re-enable preview to take a JPEG snapshot in H.264 mode",
                                        );
                                    } else {
                                        match save_snapshot(&hub, Path::new(&rec_dir)) {
                                            Ok(p) => push_state_log(
                                                &state,
                                                "info",
                                                format!("Snapshot saved by button -> {}", p.display()),
                                            ),
                                            Err(e) => push_state_log(&state, "warn", e),
                                        }
                                    }
                                }
                                ButtonAction::None => {}
                            }
                        }
                        McuEvent::HdmiSignalChange(code) => {
                            push_state_log(
                                &state,
                                "info",
                                format!("MCU reported HDMI signal change (event {code})"),
                            );
                        }
                        McuEvent::AudioSourceToggle => {
                            push_state_log(&state, "info", "MCU reported audio source switch");
                        }
                        _ => {}
                    }
                }

                // Refresh HDMI timing / audio info & auto-recover on BIOS->OS resolution change
                match dev.read_signal() {
                    Ok(sig) => {
                        let (prev_sig, prev_out_w, prev_out_h, cfg_mode) = {
                            let mut g = state.lock().unwrap();
                            let prev = g.signal;
                            g.signal = sig;
                            (prev, g.out_width, g.out_height, g.settings.mode)
                        };

                        let changed_res = hdmi_signal_changed(prev_sig, sig);
                        if streaming && changed_res {
                            if let Some(new_sig) = sig {
                                push_state_log(
                                    &state,
                                    "info",
                                    format!(
                                        "HDMI mode changed -> {}x{}@{}Hz — restarting hardware encoder and output sinks cleanly",
                                        new_sig.width, new_sig.height, new_sig.refresh_hz
                                    ),
                                );
                                if should_finalize_recording_on_mode_change(
                                    cfg_mode,
                                    prev_out_w,
                                    prev_out_h,
                                    prev_sig,
                                    &new_sig,
                                ) {
                                    stop_recording_if_active(&state);
                                }
                                let preserved_v4l2 = active_outputs
                                    .take()
                                    .and_then(ActiveOutputs::take_owned_v4l2_device);
                                hub.reset_cache();
                                let _ = dev.stop_encoder();
                                streaming = false;
                                bytes_since_tick = 0;
                                last_rate_tick = Instant::now();
                                start_capture_with_signal(
                                    dev,
                                    Some(new_sig),
                                    &state,
                                    &hub,
                                    &vu,
                                    &monitor,
                                    &mut active_outputs,
                                    &mut streaming,
                                    &mut ts_deframer,
                                    &mut mjpeg_deframer,
                                    preserved_v4l2,
                                    window_visible,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        push_state_log(&state, "warn", format!("Device poll error: {e}"));
                        stop_recording_if_active(&state);
                        active_outputs = None;
                        hub.reset_cache();
                        streaming = false;
                        bytes_since_tick = 0;
                        last_rate_tick = Instant::now();
                        device = None;
                        let mut g = state.lock().unwrap();
                        clear_streaming_state(&mut g);
                        g.phase = EnginePhase::Error;
                        g.status_message = format!("Device poll error: {e}");
                    }
                }
            }
        }
    }

    // Guaranteed teardown whether exited via EngineCmd::Shutdown or stop_flag:
    // stop recording (remux .ts -> .mp4), drop V4L2/PipeWire/SRT/HTTP sinks, and stop hardware encoder.
    stop_recording_if_active(&state);
    drop(active_outputs.take());
    if let Some(ref dev) = device {
        if streaming {
            let _ = dev.stop_encoder();
        }
    }
}

fn hdmi_signal_changed(prev: Option<HdmiSignal>, next: Option<HdmiSignal>) -> bool {
    match (prev, next) {
        (Some(a), Some(b)) => {
            a.width != b.width
                || a.height != b.height
                || a.refresh_hz != b.refresh_hz
                || a.interlaced != b.interlaced
        }
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Returns true when an active H.264 recording must be finalized before restarting the hardware encoder.
/// Stops cleanly on dimension changes or any H.264 mode change (restarting the hardware encoder resets
/// MPEG-TS PCR/PTS timestamps and would otherwise corrupt lossless `.ts -> .mp4` remux).
fn should_finalize_recording_on_mode_change(
    mode: CaptureMode,
    prev_out_width: u32,
    prev_out_height: u32,
    prev_sig: Option<HdmiSignal>,
    new_sig: &HdmiSignal,
) -> bool {
    if mode != CaptureMode::H264Aac {
        return false;
    }
    let dims_changed = prev_out_width != new_sig.width
        || prev_out_height != new_sig.height
        || prev_sig.is_some_and(|p| p.width != new_sig.width || p.height != new_sig.height);
    dims_changed || hdmi_signal_changed(prev_sig, Some(*new_sig))
}

fn clear_streaming_state(g: &mut EngineState) {
    g.out_width = 0;
    g.out_height = 0;
    g.out_fps = 0;
    g.bitrate_mbps = 0.0;
    g.http_url = None;
    g.mjpeg_url = None;
    g.snapshot_url = None;
    g.srt_url = None;
    g.v4l2_active = false;
    g.pipewire_active = false;
    g.preview_active = false;
}

fn desired_preview_active(
    cfg: &Settings,
    window_visible: bool,
    outputs: &ActiveOutputs,
) -> bool {
    if !cfg.enable_preview {
        return false;
    }
    match cfg.mode {
        CaptureMode::Mjpeg => true,
        CaptureMode::H264Aac => window_visible || outputs.external_mjpeg_clients() > 0,
    }
}

fn sync_preview_bridge(
    state: &Arc<Mutex<EngineState>>,
    hub: &StreamHub,
    active_outputs: &mut Option<ActiveOutputs>,
    window_visible: bool,
) {
    let cfg = state.lock().unwrap().settings.clone();
    let Some(outputs) = active_outputs.as_mut() else {
        state.lock().unwrap().preview_active = false;
        return;
    };
    let want_active = desired_preview_active(&cfg, window_visible, outputs);
    let log_state = Arc::clone(state);
    let log_sink: LogSink = Arc::new(move |lvl, msg| {
        push_state_log(&log_state, lvl, msg);
    });
    let bridge_changed = outputs.set_preview_running(want_active, cfg.mode, hub, Some(log_sink));
    let actual_active = match cfg.mode {
        CaptureMode::H264Aac => outputs.preview.is_some(),
        CaptureMode::Mjpeg => cfg.enable_preview,
    };
    let prev_active = {
        let mut g = state.lock().unwrap();
        let prev = g.preview_active;
        g.preview_active = actual_active;
        prev
    };
    if bridge_changed || prev_active != actual_active {
        let msg = if actual_active {
            if !window_visible && cfg.mode == CaptureMode::H264Aac {
                "Preview woke up for an external HTTP /mjpeg client"
            } else {
                "Video preview enabled"
            }
        } else if !cfg.enable_preview {
            "Video preview disabled (CPU saving — OBS/V4L2/REC unchanged)"
        } else {
            "Preview paused (window minimized to system tray)"
        };
        push_state_log(state, "info", msg);
    }
}

fn apply_active_outputs_to_state(
    g: &mut EngineState,
    cfg: &Settings,
    outputs: &ActiveOutputs,
    display_host: String,
) {
    let http_url = outputs
        .http
        .as_ref()
        .map(|_| format!("http://{display_host}:{}/stream.ts", cfg.http_port));
    let mjpeg_url = outputs
        .http
        .as_ref()
        .map(|_| format!("http://{display_host}:{}/mjpeg", cfg.http_port));
    let snapshot_url = outputs
        .http
        .as_ref()
        .map(|_| format!("http://{display_host}:{}/snapshot.jpg", cfg.http_port));
    let srt_url = outputs.srt.as_ref().map(|_| {
        format!(
            "srt://{display_host}:{}?mode=caller&transtype=live&latency=120000",
            cfg.srt_port
        )
    });
    g.display_host = display_host;
    g.http_url = http_url;
    g.mjpeg_url = mjpeg_url;
    g.snapshot_url = snapshot_url;
    g.srt_url = srt_url;
    g.v4l2_active = outputs.v4l2.is_some();
    g.pipewire_active = outputs.pipewire.is_some();
    g.preview_active = match cfg.mode {
        CaptureMode::H264Aac => outputs.preview.is_some(),
        CaptureMode::Mjpeg => cfg.enable_preview,
    };
}

#[allow(clippy::too_many_arguments)]
fn start_capture_internal(
    dev: &Gl310Device,
    state: &Arc<Mutex<EngineState>>,
    hub: &StreamHub,
    vu: &VuMeter,
    monitor: &MonitorControl,
    active_outputs: &mut Option<ActiveOutputs>,
    streaming: &mut bool,
    ts_deframer: &mut TsDeframer,
    mjpeg_deframer: &mut MjpegDeframer,
    preserved_v4l2: Option<String>,
    window_visible: bool,
) {
    let sig = dev.read_signal().ok().flatten();
    start_capture_with_signal(
        dev,
        sig,
        state,
        hub,
        vu,
        monitor,
        active_outputs,
        streaming,
        ts_deframer,
        mjpeg_deframer,
        preserved_v4l2,
        window_visible,
    );
}

#[allow(clippy::too_many_arguments)]
fn start_capture_with_signal(
    dev: &Gl310Device,
    sig: Option<HdmiSignal>,
    state: &Arc<Mutex<EngineState>>,
    hub: &StreamHub,
    vu: &VuMeter,
    monitor: &MonitorControl,
    active_outputs: &mut Option<ActiveOutputs>,
    streaming: &mut bool,
    ts_deframer: &mut TsDeframer,
    mjpeg_deframer: &mut MjpegDeframer,
    preserved_v4l2: Option<String>,
    window_visible: bool,
) {
    let cfg = state.lock().unwrap().settings.clone();

    if let Some(ref s) = sig {
        if s.audio.compressed_non_pcm && cfg.audio_input == AudioInput::Hdmi {
            push_state_log(
                state,
                "warn",
                "HDMI source is sending compressed audio (Dolby/DTS) — switch source audio to LPCM Stereo for GL310 capture",
            );
        }
        if s.hdcp {
            push_state_log(
                state,
                "warn",
                "HDCP detected on HDMI IN — disable HDCP on the source console/PC if screen stays blank",
            );
        }
    }

    *ts_deframer = TsDeframer::new();
    *mjpeg_deframer = MjpegDeframer::new();
    hub.reset_cache();

    match dev.start_encoder(&cfg, sig.as_ref()) {
        Ok(regs) => {
            let log_state = Arc::clone(state);
            let log_sink: LogSink = Arc::new(move |lvl, msg| {
                push_state_log(&log_state, lvl, msg);
            });
            let start_preview = cfg.enable_preview && window_visible;
            let outputs = ActiveOutputs::start_all_with_owned_logged(
                &cfg,
                hub,
                vu.clone(),
                monitor.clone(),
                start_preview,
                preserved_v4l2,
                Some(log_sink),
            );
            let warns = outputs.warnings.lock().unwrap().clone();
            for w in warns {
                push_state_log(state, "warn", w);
            }
            let display_host = display_host_for(cfg.expose_lan);

            let mut g = state.lock().unwrap();
            apply_active_outputs_to_state(&mut g, &cfg, &outputs, display_host);
            g.phase = EnginePhase::Streaming;
            g.signal = sig;
            g.out_width = regs.out_width;
            g.out_height = regs.out_height;
            g.out_fps = regs.out_fps;
            g.stream_generation = g.stream_generation.wrapping_add(1);
            g.status_message = format!(
                "Streaming {:?} ({}x{} @ {} fps)",
                cfg.mode, regs.out_width, regs.out_height, regs.out_fps
            );
            drop(g);

            *active_outputs = Some(outputs);
            *streaming = true;
            let sig_summary = match sig {
                Some(s) => format!(
                    "HDMI {}x{}{}@{}Hz HDCP={}",
                    s.width,
                    s.height,
                    if s.interlaced { "i" } else { "p" },
                    s.refresh_hz,
                    s.hdcp
                ),
                None => "NO HDMI SIGNAL".to_string(),
            };
            push_state_log(
                state,
                "info",
                format!(
                    "Encoder started: {:?} {}x{}@{} fps | {} | v4l2={} pw={} http={} srt={} preview={}",
                    cfg.mode,
                    regs.out_width,
                    regs.out_height,
                    regs.out_fps,
                    sig_summary,
                    active_outputs.as_ref().is_some_and(|o| o.v4l2.is_some()),
                    active_outputs.as_ref().is_some_and(|o| o.pipewire.is_some()),
                    active_outputs.as_ref().is_some_and(|o| o.http.is_some()),
                    active_outputs.as_ref().is_some_and(|o| o.srt.is_some()),
                    active_outputs.as_ref().is_some_and(|o| o.preview.is_some()),
                ),
            );
        }
        Err(e) => {
            if let Some(dev_path) = preserved_v4l2 {
                remove_v4l2_device(&dev_path);
            }
            push_state_log(state, "error", format!("Failed starting encoder: {e}"));
            let mut g = state.lock().unwrap();
            clear_streaming_state(&mut g);
            g.phase = EnginePhase::Error;
            g.status_message = format!("Start failed: {e}");
        }
    }
}

fn stop_recording_if_active(state: &Arc<Mutex<EngineState>>) {
    let rec = state.lock().unwrap().recorder.take();
    if let Some(r) = rec {
        let summary = r.stop();
        push_state_log(
            state,
            "info",
            format!("Recording finalized -> {}", summary.final_path),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::AudioSignalInfo;

    fn sample_signal(width: u32, height: u32, refresh_hz: u32) -> HdmiSignal {
        HdmiSignal {
            width,
            height,
            h_total: width + 200,
            v_total: height + 45,
            refresh_hz,
            interlaced: false,
            hdcp: false,
            audio: AudioSignalInfo {
                present: true,
                sample_rate_hz: 48_000,
                compressed_non_pcm: false,
                raw_status: 0x02,
            },
            matched_mode_index: None,
        }
    }

    #[test]
    fn live_ui_settings_do_not_require_encoder_restart() {
        let base = Settings::default();

        // Changing button_action, led_mode, pc_monitor, enable_preview, ui_language, or recordings_dir must NOT abort recordings
        let mut live = base.clone();
        live.button_action = ButtonAction::Snapshot;
        live.led_mode = LedMode::Clockwise;
        live.pc_monitor = true;
        live.pc_monitor_volume = 55;
        live.pc_monitor_muted = true;
        live.enable_preview = false;
        live.ui_language = UiLanguage::Fr;
        live.recordings_dir = "/tmp/gl310-recs".to_string();
        assert!(!settings_require_restart(&base, &live));

        // Changing encoder or output settings MUST trigger restart
        let mut enc = base.clone();
        enc.video_kbps = 12_000;
        assert!(settings_require_restart(&base, &enc));

        let mut mode_chg = base.clone();
        mode_chg.mode = CaptureMode::Mjpeg;
        assert!(settings_require_restart(&base, &mode_chg));

        let mut jack = base.clone();
        jack.audio_out_jack = !base.audio_out_jack;
        assert!(settings_require_restart(&base, &jack));
    }

    #[test]
    fn display_host_never_returns_unspecified_zero_address() {
        assert_eq!(display_host_for(false), "127.0.0.1");

        let lan = display_host_for(true);
        assert_ne!(lan, "0.0.0.0", "LAN display host must never be 0.0.0.0");
        let parsed: std::net::Ipv4Addr = lan
            .parse()
            .expect("display_host_for(true) must return a valid IPv4 address");
        assert!(!parsed.is_unspecified());
    }

    #[test]
    fn hdmi_mode_change_and_recording_finalization_rules() {
        let s720p60 = sample_signal(1280, 720, 60);
        let s1080p60 = sample_signal(1920, 1080, 60);
        let mut s1080i60 = sample_signal(1920, 1080, 60);
        s1080i60.interlaced = true;
        let s1080p24 = sample_signal(1920, 1080, 24);
        let s800x600 = sample_signal(800, 600, 60);

        // Identical signal -> no change
        assert!(!hdmi_signal_changed(Some(s1080p60), Some(s1080p60)));
        // Signal lost -> do not tear down until lock returns
        assert!(!hdmi_signal_changed(Some(s1080p60), None));
        assert!(!hdmi_signal_changed(None, None));
        // Signal acquired or changed (including interlaced <-> progressive) -> trigger reconfigure
        assert!(hdmi_signal_changed(None, Some(s720p60)));
        assert!(hdmi_signal_changed(Some(s720p60), Some(s1080p60)));
        assert!(hdmi_signal_changed(Some(s800x600), Some(s1080p60)));
        assert!(hdmi_signal_changed(Some(s1080p60), Some(s1080p24)));
        assert!(hdmi_signal_changed(Some(s1080i60), Some(s1080p60)));
        assert!(hdmi_signal_changed(Some(s1080p60), Some(s1080i60)));

        // H.264 recording finalizes on dimension or mode change; MJPEG never records
        assert!(should_finalize_recording_on_mode_change(
            CaptureMode::H264Aac,
            1280,
            720,
            Some(s720p60),
            &s1080p60
        ));
        assert!(should_finalize_recording_on_mode_change(
            CaptureMode::H264Aac,
            1920,
            1080,
            None,
            &s720p60
        ));
        assert!(should_finalize_recording_on_mode_change(
            CaptureMode::H264Aac,
            1920,
            1080,
            Some(s1080p60),
            &s1080p24
        ));
        assert!(should_finalize_recording_on_mode_change(
            CaptureMode::H264Aac,
            1920,
            1080,
            Some(s1080i60),
            &s1080p60
        ));
        assert!(!should_finalize_recording_on_mode_change(
            CaptureMode::H264Aac,
            1920,
            1080,
            Some(s1080p60),
            &s1080p60
        ));
        assert!(!should_finalize_recording_on_mode_change(
            CaptureMode::Mjpeg,
            1280,
            720,
            Some(s720p60),
            &s1080p60
        ));
    }

    #[test]
    fn apply_active_outputs_formats_lan_urls_and_clear_resets_state() {
        let hub = StreamHub::new();
        let vu = VuMeter::new();
        let cfg = Settings {
            mode: CaptureMode::H264Aac,
            enable_v4l2: false,
            enable_pipewire_audio: false,
            enable_srt: true,
            srt_port: 14_870,
            enable_http: true,
            http_port: 14_868,
            expose_lan: true,
            ..Default::default()
        };
        let monitor = MonitorControl::from_settings(&cfg);
        let outputs = ActiveOutputs::start_all(&cfg, &hub, vu, monitor, false);

        let mut st = EngineState {
            phase: EnginePhase::Idle,
            status_message: String::new(),
            device_info: None,
            signal: None,
            out_width: 1920,
            out_height: 1080,
            out_fps: 30,
            bitrate_mbps: 9.5,
            settings: cfg.clone(),
            firmware: inspect_firmware_status(None),
            stream_generation: 0,
            logs: VecDeque::new(),
            recorder: None,
            display_host: "127.0.0.1".into(),
            http_url: None,
            mjpeg_url: None,
            snapshot_url: None,
            srt_url: None,
            v4l2_active: false,
            pipewire_active: false,
            preview_active: false,
        };

        let host = display_host_for(cfg.expose_lan);
        apply_active_outputs_to_state(&mut st, &cfg, &outputs, host.clone());
        assert_eq!(st.display_host, host);
        assert_ne!(st.display_host, "0.0.0.0");
        assert_eq!(
            st.http_url.as_deref(),
            Some(format!("http://{host}:14868/stream.ts").as_str())
        );
        assert_eq!(
            st.mjpeg_url.as_deref(),
            Some(format!("http://{host}:14868/mjpeg").as_str())
        );
        assert_eq!(
            st.snapshot_url.as_deref(),
            Some(format!("http://{host}:14868/snapshot.jpg").as_str())
        );
        assert_eq!(
            st.srt_url.as_deref(),
            Some(format!("srt://{host}:14870?mode=caller&transtype=live&latency=120000").as_str())
        );
        assert!(!st.v4l2_active);
        assert!(!st.pipewire_active);

        // Re-applying with a refreshed LAN IP resyncs all formatted URLs consistently
        apply_active_outputs_to_state(&mut st, &cfg, &outputs, "192.168.50.10".into());
        assert_eq!(st.display_host, "192.168.50.10");
        assert_eq!(
            st.srt_url.as_deref(),
            Some("srt://192.168.50.10:14870?mode=caller&transtype=live&latency=120000")
        );
        assert_eq!(
            st.http_url.as_deref(),
            Some("http://192.168.50.10:14868/stream.ts")
        );

        clear_streaming_state(&mut st);
        assert_eq!(st.out_width, 0);
        assert_eq!(st.out_height, 0);
        assert_eq!(st.out_fps, 0);
        assert_eq!(st.bitrate_mbps, 0.0);
        assert!(st.http_url.is_none());
        assert!(st.mjpeg_url.is_none());
        assert!(st.snapshot_url.is_none());
        assert!(st.srt_url.is_none());
        assert!(!st.v4l2_active);
        assert!(!st.pipewire_active);
        assert!(!st.preview_active);

        drop(outputs);
    }

    #[test]
    fn preview_bridge_toggles_dynamically_and_pauses_when_window_hidden() {
        let hub = StreamHub::new();
        let vu = VuMeter::new();
        let cfg = Settings {
            mode: CaptureMode::H264Aac,
            enable_v4l2: false,
            enable_pipewire_audio: false,
            enable_srt: false,
            enable_http: true,
            http_port: 0,
            enable_preview: true,
            ..Default::default()
        };
        let monitor = MonitorControl::from_settings(&cfg);
        let mut outputs = ActiveOutputs::start_all(&cfg, &hub, vu, monitor, true);
        assert!(outputs.preview.is_some());
        assert!(desired_preview_active(&cfg, true, &outputs));
        assert!(!desired_preview_active(&cfg, false, &outputs));

        // Minimize to tray with no external /mjpeg clients -> preview pauses cleanly
        assert!(outputs.set_preview_running(false, cfg.mode, &hub, None));
        assert!(outputs.preview.is_none());

        // User explicitly disables preview -> remains off even when window is visible
        let mut cfg_off = cfg.clone();
        cfg_off.enable_preview = false;
        assert!(!desired_preview_active(&cfg_off, true, &outputs));

        // Re-enabling starts PreviewBridge dynamically without touching HttpServer
        let http_addr = outputs.http.as_ref().unwrap().address().to_string();
        assert!(outputs.set_preview_running(true, cfg.mode, &hub, None));
        assert!(outputs.preview.is_some());
        assert_eq!(outputs.http.as_ref().unwrap().address(), http_addr);

        drop(outputs);
    }

    #[test]
    #[ignore = "Requires plugged-in AVerMedia GL310 (07ca:c835) with active HDMI signal"]
    fn live_hardware_preview_toggle_and_tray_pause() {
        let cfg = Settings {
            mode: CaptureMode::H264Aac,
            enable_v4l2: true,
            enable_pipewire_audio: true,
            enable_srt: false,
            enable_http: true,
            http_port: 14_898,
            enable_preview: true,
            ..Default::default()
        };
        let engine = CaptureEngine::start(Some(cfg));

        // Wait for device probe / RAM boot to reach Idle
        let mut ready = false;
        for _ in 0..80 {
            if engine.snapshot().phase == EnginePhase::Idle {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "GL310 did not reach Idle: {:?}", engine.snapshot().status_message);

        // 1. Start capture with preview ON
        let snap1 = engine.start_capture_sync().expect("start_capture_sync");
        assert_eq!(snap1.phase, EnginePhase::Streaming);
        assert!(snap1.preview_active);
        assert!(snap1.v4l2_active);
        let gen0 = snap1.stream_generation;

        // Wait for live preview frames + TS packets
        let mut got_frames = false;
        for _ in 0..50 {
            let s = engine.snapshot();
            if s.mjpeg_frames > 0 && s.ts_stats.packets > 0 {
                got_frames = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(got_frames, "Expected live preview JPEG frames after capture start");

        // 2. Toggle preview OFF live -> zero encoder restart, V4L2/PW stay active, TS keeps flowing
        let pkts_before_off = engine.snapshot().ts_stats.packets;
        let snap_off = engine.set_preview_enabled(false).expect("set_preview_enabled(false)");
        assert!(!snap_off.preview_active);
        assert!(!snap_off.settings.enable_preview);
        assert_eq!(snap_off.stream_generation, gen0, "Toggling preview OFF must NOT restart encoder");
        assert!(snap_off.v4l2_active);
        assert!(snap_off.pipewire_active);
        let snap_err = engine.take_snapshot().unwrap_err();
        assert!(snap_err.contains("Preview disabled"), "Unexpected snapshot err: {snap_err}");

        thread::sleep(Duration::from_millis(600));
        let s_during_off = engine.snapshot();
        assert!(
            s_during_off.ts_stats.packets > pkts_before_off,
            "MPEG-TS stream must keep flowing while preview is OFF"
        );
        assert_eq!(s_during_off.mjpeg_frames, 0, "PreviewBridge must not publish frames while OFF");

        // 3. Toggle preview back ON live -> zero encoder restart, frames resume
        let snap_on = engine.set_preview_enabled(true).expect("set_preview_enabled(true)");
        assert!(snap_on.preview_active);
        assert!(snap_on.settings.enable_preview);
        assert_eq!(snap_on.stream_generation, gen0, "Toggling preview ON must NOT restart encoder");
        let mut resumed = false;
        for _ in 0..50 {
            if engine.snapshot().mjpeg_frames > 0 {
                resumed = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(resumed, "Preview frames did not resume after re-enabling preview");

        // 4. Hide to system tray -> PreviewBridge auto-pauses with zero encoder restart
        engine.set_window_visible(false);
        let mut tray_paused = false;
        for _ in 0..20 {
            if !engine.snapshot().preview_active {
                tray_paused = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(tray_paused, "PreviewBridge should auto-pause when window is hidden in tray");
        assert_eq!(engine.snapshot().stream_generation, gen0);
        assert!(engine.snapshot().v4l2_active);

        // 5. Restore from system tray -> PreviewBridge wakes back up automatically
        engine.set_window_visible(true);
        let mut tray_resumed = false;
        for _ in 0..50 {
            let s = engine.snapshot();
            if s.preview_active && s.mjpeg_frames > 0 {
                tray_resumed = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(tray_resumed, "PreviewBridge did not wake up on tray restore");
        assert_eq!(engine.snapshot().stream_generation, gen0);

        let _ = engine.stop_capture_sync();
        engine.shutdown();
    }

    #[test]
    fn capture_engine_shutdown_is_clean_and_idempotent() {
        let lan_cfg = Settings {
            expose_lan: true,
            ..Default::default()
        };
        let engine = CaptureEngine::start(Some(lan_cfg));
        let snap = engine.snapshot();
        assert_eq!(snap.stream_generation, 0);
        assert_ne!(snap.display_host, "0.0.0.0");
        assert_eq!(snap.settings.ui_language, UiLanguage::En);

        // Switching language updates snapshot immediately without restarting anything
        let snap_fr = engine.set_ui_language(UiLanguage::Fr).unwrap();
        assert_eq!(snap_fr.settings.ui_language, UiLanguage::Fr);
        let snap_en = engine.set_ui_language(UiLanguage::En).unwrap();
        assert_eq!(snap_en.settings.ui_language, UiLanguage::En);

        // Snapshot while preview is disabled in H.264 returns a clear English message
        let _ = engine.set_preview_enabled(false).unwrap();
        let err = engine.take_snapshot().unwrap_err();
        assert!(err.contains("Preview disabled"), "Unexpected error: {err}");

        let t0 = Instant::now();
        engine.shutdown();
        engine.shutdown(); // idempotent second call
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "CaptureEngine::shutdown took too long: {:?}",
            t0.elapsed()
        );
    }
}
