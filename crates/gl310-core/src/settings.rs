//! User configuration, validation, and JSON persistence for GL310-Linux.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const VIDEO_KBPS_MIN: u32 = 4_000;
pub const VIDEO_KBPS_MAX: u32 = 30_000;
pub const VIDEO_KBPS_PRESETS: [u32; 6] = [4_000, 6_000, 8_000, 12_000, 20_000, 30_000];
pub const AUDIO_KBPS_PRESETS: [u32; 4] = [128, 160, 192, 256];
pub const ANALOG_VOLUME_MIN: u8 = 1;
pub const ANALOG_VOLUME_MAX: u8 = 11;
pub const ENCODER_VOLUME_MAX: u8 = 15;
pub const MJPEG_QUALITY_MAX: u8 = 15;

/// Hardware capture codec mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// H.264 + AAC multiplexed in MPEG-TS (188-byte packets) — ideal for OBS, preview, and .mp4 recording.
    H264Aac,
    /// Native hardware MJPEG frames — zero CPU decoding, low latency, tailored for Raspberry Pi KVM / PiKVM.
    Mjpeg,
}

/// Output frame rate policy for the hardware encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FpsPolicy {
    /// Automatic: 1080p -> 30 fps (hardware limit), 720p & below -> native up to 60 fps.
    Auto,
    /// Force encoder output to 30 fps regardless of input (matches 1080p30 fixed workflow).
    Force30,
    /// Force encoder output to 25 fps (PAL workflows).
    Force25,
    /// Force encoder output to 24 fps (cinema workflows).
    Force24,
    /// Force encoder output to 15 fps (low-bandwidth KVM).
    Force15,
}

impl FpsPolicy {
    /// Resolve target encoder output fps given the detected HDMI input refresh rate and height.
    pub fn resolve(self, input_height: u32, input_hz: u32) -> u32 {
        let cap = if input_height >= 1080 { 30 } else { 60 };
        let native = input_hz.clamp(15, cap);
        match self {
            FpsPolicy::Auto => native,
            FpsPolicy::Force30 => 30.min(input_hz.max(15)),
            FpsPolicy::Force25 => 25.min(input_hz.max(15)),
            FpsPolicy::Force24 => 24.min(input_hz.max(15)),
            FpsPolicy::Force15 => 15,
        }
    }
}

/// GOP / keyframe interval policy in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GopSeconds {
    OneSec,
    TwoSec,
}

impl GopSeconds {
    pub fn frames(self, out_fps: u32) -> u32 {
        let secs = match self {
            GopSeconds::OneSec => 1,
            GopSeconds::TwoSec => 2,
        };
        (out_fps.clamp(15, 60) * secs).clamp(15, 120)
    }
}

/// Hardware MJPEG output resolution presets supported by the GL310 scaler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MjpegResolution {
    R640x360,
    R960x540,
    R1280x720,
    R1920x1080,
}

impl MjpegResolution {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            MjpegResolution::R640x360 => (640, 360),
            MjpegResolution::R960x540 => (960, 540),
            MjpegResolution::R1280x720 => (1280, 720),
            MjpegResolution::R1920x1080 => (1920, 1080),
        }
    }

    pub fn from_label(label: &str) -> Result<Self, String> {
        match label.to_ascii_lowercase().as_str() {
            "360p" | "640x360" => Ok(Self::R640x360),
            "540p" | "960x540" => Ok(Self::R960x540),
            "720p" | "1280x720" => Ok(Self::R1280x720),
            "1080p" | "1920x1080" => Ok(Self::R1920x1080),
            other => Err(format!(
                "Invalid MJPEG resolution '{other}' (expected 360p, 540p, 720p, or 1080p)"
            )),
        }
    }
}

/// Hardware audio input source selected via MCU register 0x2E.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioInput {
    /// Embedded HDMI LPCM audio (MCU 0x2E = 0x04).
    Hdmi,
    /// Rear 3.5 mm analog stereo line-in jack (MCU 0x2E = 0x05).
    Jack,
}

/// V4L2 virtual camera scaling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum V4l2FormatPolicy {
    /// Match the incoming HDMI / encoder resolution and fps automatically.
    Auto,
    /// Force fixed 1920x1080 @ 30 fps output on /dev/video14 (for OBS scene stability).
    Fixed1080p30,
}

/// Hardware LED ring animation modes (MCU register 0x2C).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedMode {
    /// Automatically reflects capture state (idle/streaming/recording/error).
    Auto,
    /// Clockwise spinning blue ring — idle / power-on pattern (MCU 0x2C = 0x04).
    Clockwise,
    /// Solid ring — active capture / recording pattern (MCU 0x2C = 0x05).
    Solid,
    /// Slow breathing / fade pulse (MCU 0x2C = 0x03).
    Fade,
    /// Fast clockwise feedback flash (MCU 0x2C = 0x0B).
    Fast,
}

impl LedMode {
    pub fn mcu_code(self) -> Option<u8> {
        match self {
            LedMode::Auto => None,
            LedMode::Fade => Some(0x03),
            LedMode::Clockwise => Some(0x04),
            LedMode::Solid => Some(0x05),
            LedMode::Fast => Some(0x0B),
        }
    }

    pub fn from_label(label: &str) -> Result<Self, String> {
        match label.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "clockwise" | "spin" => Ok(Self::Clockwise),
            "solid" | "on" => Ok(Self::Solid),
            "fade" | "pulse" | "breathe" => Ok(Self::Fade),
            "fast" | "blink" => Ok(Self::Fast),
            other => Err(format!(
                "Invalid LED mode '{other}' (expected auto, clockwise, solid, fade, or fast)"
            )),
        }
    }
}

/// Action triggered when the physical center button on the GL310 is clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    ToggleRecord,
    ToggleCapture,
    Snapshot,
    None,
}

/// User interface language for GL310 Studio (English by default, live switchable to French).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiLanguage {
    En,
    Fr,
}

/// Complete validated settings for a GL310 capture session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: CaptureMode,
    pub fps_policy: FpsPolicy,
    pub video_kbps: u32,
    pub gop: GopSeconds,
    pub mjpeg_resolution: MjpegResolution,
    pub mjpeg_fps: u32,
    pub mjpeg_quality: u8,

    pub audio_input: AudioInput,
    /// Analog 3.5 mm Jack AUDIO IN gain (MCU 0x41, verified 1..=11 in AVer835_x64.inf).
    pub analog_volume: u8,
    /// Encoder digital input volume (0x6F8 + mailbox 0x0A, verified 0..=15; default 8).
    pub encoder_volume: u8,
    pub audio_kbps: u32,
    /// Hardware 3.5 mm AUDIO OUT passthrough (aou_pass_en bit 16 of 0x6D4).
    pub audio_out_jack: bool,

    /// Host PC live audio monitoring (PipeWire playback of decoded audio).
    pub pc_monitor: bool,
    pub pc_monitor_volume: u8,
    pub pc_monitor_muted: bool,

    /// Output sinks.
    pub enable_v4l2: bool,
    pub v4l2_device: String,
    pub v4l2_format: V4l2FormatPolicy,
    pub enable_pipewire_audio: bool,
    pub enable_srt: bool,
    pub srt_port: u16,
    pub enable_http: bool,
    pub http_port: u16,
    pub expose_lan: bool,
    /// Studio live video preview (disabling pauses PreviewBridge H.264->MJPEG transcoding to save CPU).
    pub enable_preview: bool,
    /// Studio UI language (English by default, switchable live in the top bar).
    pub ui_language: UiLanguage,

    /// Hardware interaction & recording path.
    pub led_mode: LedMode,
    pub button_action: ButtonAction,
    pub recordings_dir: String,
    pub firmware_dir_override: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let default_rec_dir = default_recordings_dir()
            .to_string_lossy()
            .into_owned();
        Self {
            mode: CaptureMode::H264Aac,
            fps_policy: FpsPolicy::Auto,
            video_kbps: 8_000,
            gop: GopSeconds::TwoSec,
            mjpeg_resolution: MjpegResolution::R1280x720,
            mjpeg_fps: 30,
            mjpeg_quality: 11,
            audio_input: AudioInput::Hdmi,
            analog_volume: 8,
            encoder_volume: 8,
            audio_kbps: 192,
            audio_out_jack: true,
            pc_monitor: false,
            pc_monitor_volume: 80,
            pc_monitor_muted: false,
            enable_v4l2: true,
            v4l2_device: "/dev/video14".to_string(),
            v4l2_format: V4l2FormatPolicy::Auto,
            enable_pipewire_audio: true,
            enable_srt: true,
            srt_port: 4850,
            enable_http: true,
            http_port: 4848,
            expose_lan: false,
            enable_preview: true,
            ui_language: UiLanguage::En,
            led_mode: LedMode::Auto,
            button_action: ButtonAction::ToggleRecord,
            recordings_dir: default_rec_dir,
            firmware_dir_override: None,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(VIDEO_KBPS_MIN..=VIDEO_KBPS_MAX).contains(&self.video_kbps) {
            return Err(format!(
                "video_kbps {} out of range ({}..={})",
                self.video_kbps, VIDEO_KBPS_MIN, VIDEO_KBPS_MAX
            ));
        }
        if !AUDIO_KBPS_PRESETS.contains(&self.audio_kbps) {
            return Err(format!(
                "audio_kbps {} invalid (supported: {:?})",
                self.audio_kbps, AUDIO_KBPS_PRESETS
            ));
        }
        if !(ANALOG_VOLUME_MIN..=ANALOG_VOLUME_MAX).contains(&self.analog_volume) {
            return Err(format!(
                "analog_volume {} out of range ({}..={})",
                self.analog_volume, ANALOG_VOLUME_MIN, ANALOG_VOLUME_MAX
            ));
        }
        if self.encoder_volume > ENCODER_VOLUME_MAX {
            return Err(format!(
                "encoder_volume {} out of range (0..={})",
                self.encoder_volume, ENCODER_VOLUME_MAX
            ));
        }
        if self.mjpeg_quality > MJPEG_QUALITY_MAX {
            return Err(format!(
                "mjpeg_quality {} out of range (0..={})",
                self.mjpeg_quality, MJPEG_QUALITY_MAX
            ));
        }
        if !matches!(self.mjpeg_fps, 15 | 24 | 25 | 30) {
            return Err(format!(
                "mjpeg_fps {} unsupported (expected 15, 24, 25, or 30)",
                self.mjpeg_fps
            ));
        }
        if self.pc_monitor_volume > 100 {
            return Err(format!(
                "pc_monitor_volume {} out of range (0..=100)",
                self.pc_monitor_volume
            ));
        }
        if self.http_port == 0 || self.srt_port == 0 {
            return Err("Network ports must be non-zero".to_string());
        }
        if !self.v4l2_device.starts_with("/dev/video") {
            return Err(format!(
                "v4l2_device '{}' must point to a /dev/videoN node",
                self.v4l2_device
            ));
        }
        Ok(())
    }

    pub fn bind_host(&self) -> &'static str {
        if self.expose_lan {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }

    pub fn load_or_default(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Settings>(&raw) {
                Ok(cfg) if cfg.validate().is_ok() => cfg,
                _ => Settings::default(),
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir {}: {e}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {e}"))?;
        fs::write(path, body)
            .map_err(|e| format!("Failed to write settings {}: {e}", path.display()))
    }
}

pub fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("gl310-linux").join("settings.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("gl310-linux")
            .join("settings.json");
    }
    PathBuf::from("gl310-settings.json")
}

pub fn default_recordings_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let videos = PathBuf::from(&home).join("Videos").join("GL310");
        return videos;
    }
    PathBuf::from("./recordings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_valid() {
        let s = Settings::default();
        assert!(s.validate().is_ok());
        assert_eq!(s.bind_host(), "127.0.0.1");
    }

    #[test]
    fn fps_policy_resolution_logic() {
        assert_eq!(FpsPolicy::Auto.resolve(1080, 60), 30);
        assert_eq!(FpsPolicy::Auto.resolve(1080, 24), 24);
        assert_eq!(FpsPolicy::Auto.resolve(720, 60), 60);
        assert_eq!(FpsPolicy::Auto.resolve(720, 50), 50);
        assert_eq!(FpsPolicy::Force30.resolve(720, 60), 30);
        assert_eq!(FpsPolicy::Force30.resolve(1080, 60), 30);
        assert_eq!(FpsPolicy::Force25.resolve(1080, 60), 25);
        assert_eq!(FpsPolicy::Force24.resolve(1080, 60), 24);
        assert_eq!(FpsPolicy::Force15.resolve(1080, 60), 15);
    }

    #[test]
    fn rejects_invalid_bounds() {
        let s = Settings {
            video_kbps: 500,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            analog_volume: 0,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            analog_volume: 12,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            encoder_volume: 16,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            mjpeg_quality: 16,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            audio_kbps: 64,
            ..Default::default()
        };
        assert!(s.validate().is_err());

        let s = Settings {
            v4l2_device: "/tmp/fake".to_string(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn json_roundtrip_works() {
        let s = Settings {
            mode: CaptureMode::Mjpeg,
            fps_policy: FpsPolicy::Force30,
            audio_input: AudioInput::Jack,
            analog_volume: 11,
            expose_lan: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        assert_eq!(back.bind_host(), "0.0.0.0");
    }

    #[test]
    fn serde_enum_names_match_studio_ui_ipc_contract() {
        assert_eq!(serde_json::to_string(&CaptureMode::H264Aac).unwrap(), "\"h264_aac\"");
        assert_eq!(serde_json::to_string(&CaptureMode::Mjpeg).unwrap(), "\"mjpeg\"");
        assert_eq!(serde_json::to_string(&FpsPolicy::Auto).unwrap(), "\"auto\"");
        assert_eq!(serde_json::to_string(&FpsPolicy::Force30).unwrap(), "\"force30\"");
        assert_eq!(serde_json::to_string(&FpsPolicy::Force25).unwrap(), "\"force25\"");
        assert_eq!(serde_json::to_string(&FpsPolicy::Force24).unwrap(), "\"force24\"");
        assert_eq!(serde_json::to_string(&GopSeconds::OneSec).unwrap(), "\"one_sec\"");
        assert_eq!(serde_json::to_string(&GopSeconds::TwoSec).unwrap(), "\"two_sec\"");
        assert_eq!(serde_json::to_string(&V4l2FormatPolicy::Auto).unwrap(), "\"auto\"");
        assert_eq!(serde_json::to_string(&V4l2FormatPolicy::Fixed1080p30).unwrap(), "\"fixed1080p30\"");
        assert_eq!(serde_json::to_string(&MjpegResolution::R640x360).unwrap(), "\"r640x360\"");
        assert_eq!(serde_json::to_string(&MjpegResolution::R960x540).unwrap(), "\"r960x540\"");
        assert_eq!(serde_json::to_string(&MjpegResolution::R1280x720).unwrap(), "\"r1280x720\"");
        assert_eq!(serde_json::to_string(&MjpegResolution::R1920x1080).unwrap(), "\"r1920x1080\"");
        assert_eq!(serde_json::to_string(&AudioInput::Hdmi).unwrap(), "\"hdmi\"");
        assert_eq!(serde_json::to_string(&AudioInput::Jack).unwrap(), "\"jack\"");
        assert_eq!(serde_json::to_string(&LedMode::Clockwise).unwrap(), "\"clockwise\"");
        assert_eq!(serde_json::to_string(&ButtonAction::ToggleRecord).unwrap(), "\"toggle_record\"");
        assert_eq!(serde_json::to_string(&ButtonAction::ToggleCapture).unwrap(), "\"toggle_capture\"");
        assert_eq!(serde_json::to_string(&UiLanguage::En).unwrap(), "\"en\"");
        assert_eq!(serde_json::to_string(&UiLanguage::Fr).unwrap(), "\"fr\"");
    }
}
