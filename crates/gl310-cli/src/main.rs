//! `gl310-cli`: Universal zero-GUI Linux CLI & Raspberry Pi KVM daemon for AVerMedia GL310 (`07ca:c835`).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};

use gl310_core::device::Gl310Device;
use gl310_core::firmware::{
    discover_verified_firmware, fetch_and_install_official_firmware, import_firmware_from_path,
    inspect_firmware_status,
};
use gl310_core::outputs::{ActiveOutputs, HttpServer, MonitorControl, V4l2Output, VuMeter};
use gl310_core::proto::McuEvent;
use gl310_core::recorder::TsRecorder;
use gl310_core::{display_host_for, run_doctor};
use gl310_core::settings::{
    AudioInput, CaptureMode, FpsPolicy, LedMode, MjpegResolution, Settings, V4l2FormatPolicy,
};
use gl310_core::stream::{MjpegDeframer, StreamHub, TsDeframer};

#[derive(Parser)]
#[command(
    name = "gl310-cli",
    version,
    about = "Userspace Linux driver CLI & Raspberry Pi KVM daemon for AVerMedia LGP Lite (GL310, 07ca:c835)"
)]
struct Cli {
    /// Override firmware directory containing qpvidfwusb.bin and qpaudfwusb.bin
    #[arg(long, global = true, env = "GL310_FIRMWARE")]
    firmware: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, ValueEnum)]
enum CliAudioIn {
    Hdmi,
    Jack,
}

impl From<CliAudioIn> for AudioInput {
    fn from(v: CliAudioIn) -> Self {
        match v {
            CliAudioIn::Hdmi => AudioInput::Hdmi,
            CliAudioIn::Jack => AudioInput::Jack,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum CliFps {
    Auto,
    #[value(name = "60")]
    F60,
    #[value(name = "30")]
    F30,
    #[value(name = "25")]
    F25,
    #[value(name = "24")]
    F24,
    #[value(name = "15")]
    F15,
}

impl From<CliFps> for FpsPolicy {
    fn from(v: CliFps) -> Self {
        match v {
            CliFps::Auto | CliFps::F60 => FpsPolicy::Auto,
            CliFps::F30 => FpsPolicy::Force30,
            CliFps::F25 => FpsPolicy::Force25,
            CliFps::F24 => FpsPolicy::Force24,
            CliFps::F15 => FpsPolicy::Force15,
        }
    }
}

#[derive(Subcommand)]
enum FirmwareCmd {
    /// Download official AVerMedia v3.2802.64.40 archive (MD5 + SHA-256 audited) and install firmwares
    Fetch {
        /// Custom destination directory (defaults to ~/.local/share/gl310-linux/firmware)
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    /// Import qpvidfwusb.bin & qpaudfwusb.bin from a local Windows driver folder or ZIP file
    Import {
        /// Path to Windows driver ZIP archive or directory
        source: PathBuf,
        /// Custom destination directory (defaults to ~/.local/share/gl310-linux/firmware)
        #[arg(long)]
        dest: Option<PathBuf>,
    },
    /// Verify installed firmware locations and strict SHA-256 checksums
    Verify {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Diagnose USB presence, udev permissions, firmware SHA-256, v4l2loopback, ffmpeg, and PipeWire
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Manage AVerMedia GL310 firmware files (fetch, import, verify)
    Firmware {
        #[command(subcommand)]
        action: FirmwareCmd,
    },
    /// Read hardware chip revision, MCU identity, firmware status, and audio routing
    Info {
        #[arg(long)]
        json: bool,
    },
    /// Read current HDMI video timing, audio sample rate, HDCP, and compressed-audio flags
    Signal {
        #[arg(long)]
        json: bool,
        /// Poll continuously until Ctrl+C
        #[arg(long)]
        watch: bool,
    },
    /// Phase 1 hardware dry-test (no HDMI source needed): USB, MCU, safety guard, LEDs, button, RAM QSOS boot
    DryTest {
        /// Seconds to wait for a physical button press (0 to skip)
        #[arg(long, default_value_t = 5)]
        button_wait: u64,
        #[arg(long)]
        json: bool,
    },
    /// Set the physical LED ring pattern (auto, clockwise, solid, fade, fast)
    Led {
        mode: String,
    },
    /// Listen for physical GL310 button clicks and optionally execute a command on each press
    Button {
        /// Shell command executed via `sh -c` on every physical button click
        #[arg(long)]
        exec: Option<String>,
    },
    /// Stream raw H.264+AAC MPEG-TS to stdout (pipe into ffplay -, vlc -, mpv -, etc.)
    Capture {
        #[arg(long, value_enum, default_value_t = CliFps::Auto)]
        fps: CliFps,
        #[arg(long, default_value_t = 8000)]
        bitrate: u32,
        #[arg(long, default_value_t = 192)]
        audio_bitrate: u32,
        #[arg(long, value_enum, default_value_t = CliAudioIn::Hdmi)]
        audio_in: CliAudioIn,
        /// Analog 3.5 mm Jack AUDIO IN gain (1..=11)
        #[arg(long, default_value_t = 8)]
        analog_vol: u8,
        /// Enable 3.5 mm AUDIO OUT jack passthrough (accepts --audio-out, --audio-out true, or --audio-out false)
        #[arg(long, default_value_t = true, default_missing_value = "true", num_args = 0..=1, action = clap::ArgAction::Set)]
        audio_out: bool,
    },
    /// Record H.264+AAC directly to .mp4 (lossless faststart remux) or raw .ts
    Record {
        /// Output file path (.mp4 or .ts)
        output: PathBuf,
        #[arg(long, value_enum, default_value_t = CliFps::Auto)]
        fps: CliFps,
        #[arg(long, default_value_t = 8000)]
        bitrate: u32,
        #[arg(long, default_value_t = 192)]
        audio_bitrate: u32,
        #[arg(long, value_enum, default_value_t = CliAudioIn::Hdmi)]
        audio_in: CliAudioIn,
        #[arg(long, default_value_t = 8)]
        analog_vol: u8,
        #[arg(long, default_value_t = true, default_missing_value = "true", num_args = 0..=1, action = clap::ArgAction::Set)]
        audio_out: bool,
        /// Optional auto-stop recording duration in seconds
        #[arg(long)]
        duration: Option<u64>,
    },
    /// Expose the GL310 as a standard V4L2 camera (/dev/video14) on any Linux distro
    V4l2 {
        #[arg(long, default_value = "/dev/video14")]
        device: String,
        /// Lock V4L2 output to 1920x1080 @ 30 fps
        #[arg(long)]
        force_1080p30: bool,
        /// Use hardware MJPEG mode (native zero-ffmpeg V4L2 MJPEG via ioctl)
        #[arg(long)]
        mjpeg: bool,
        /// Resolution when --mjpeg is enabled (360p, 540p, 720p, 1080p)
        #[arg(long, default_value = "720p")]
        res: String,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        #[arg(long, default_value_t = 11)]
        quality: u8,
    },
    /// Full headless OBS Studio bridge: V4L2 /dev/video14 + PipeWire 'GL310 Audio' + SRT + HTTP
    Obs {
        #[arg(long, default_value = "/dev/video14")]
        device: String,
        #[arg(long)]
        force_1080p30: bool,
        #[arg(long, default_value_t = 8000)]
        bitrate: u32,
        #[arg(long, default_value_t = 192)]
        audio_bitrate: u32,
        #[arg(long, value_enum, default_value_t = CliAudioIn::Hdmi)]
        audio_in: CliAudioIn,
        #[arg(long, default_value_t = 8)]
        analog_vol: u8,
        #[arg(long, default_value_t = true, default_missing_value = "true", num_args = 0..=1, action = clap::ArgAction::Set)]
        audio_out: bool,
        #[arg(long, default_value_t = 4850)]
        srt_port: u16,
        #[arg(long, default_value_t = 4848)]
        http_port: u16,
        /// Expose HTTP & SRT on 0.0.0.0 instead of 127.0.0.1
        #[arg(long)]
        lan: bool,
        /// Enable live PC speaker monitoring
        #[arg(long)]
        monitor: bool,
    },
    /// Low-latency Raspberry Pi KVM / PiKVM daemon in hardware MJPEG mode (zero ffmpeg CPU decode)
    Kvm {
        /// Hardware MJPEG output resolution (360p, 540p, 720p, 1080p)
        #[arg(long, default_value = "720p")]
        res: String,
        /// Target frame rate (15, 24, 25, or 30)
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// Hardware JPEG quality (0..=15, higher = sharper)
        #[arg(long, default_value_t = 12)]
        quality: u8,
        /// HTTP bind address (e.g. 0.0.0.0:4848 or 127.0.0.1:4848) serving /mjpeg, /snapshot.jpg, and viewer /
        #[arg(long, default_value = "0.0.0.0:4848")]
        http: String,
        /// Optional V4L2 MJPEG device path (e.g. /dev/video14) for uStreamer / PiKVM integration
        #[arg(long)]
        v4l2: Option<String>,
    },
    /// Generate shell autocompletion script (bash, zsh, fish)
    Completions {
        shell: Shell,
    },
}

fn setup_ctrlc() -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);
    let _ = ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    });
    running
}

fn open_and_ensure_booted(fw_override: Option<&Path>) -> Result<Gl310Device, String> {
    let dev = Gl310Device::open().map_err(|e| e.to_string())?;
    let loaded = dev.firmware_loaded().map_err(|e| e.to_string())?;
    if !loaded {
        let fw = discover_verified_firmware(fw_override)?;
        eprintln!("[gl310-cli] Booting verified firmware into ARM RAM...");
        dev.boot_firmware(&fw, |step| eprintln!("  -> {step}"))
            .map_err(|e| e.to_string())?;
        eprintln!("[gl310-cli] RAM boot complete (QSOS active).");
    }
    Ok(dev)
}

fn parse_host_port(spec: &str) -> Result<(String, u16), String> {
    let (host, port_str) = spec
        .rsplit_once(':')
        .ok_or_else(|| format!("Invalid bind address '{spec}' (expected HOST:PORT)"))?;
    let port = port_str
        .parse::<u16>()
        .map_err(|_| format!("Invalid port in '{spec}'"))?;
    Ok((host.to_string(), port))
}

fn run(cli: Cli) -> Result<(), String> {
    let fw_ov = cli.firmware.as_deref();

    match cli.command {
        Commands::Doctor { json } => {
            let report = run_doctor(fw_ov);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            println!("=== GL310-Linux System Doctor ===");
            if report.usb.present {
                let bus = report.usb.bus.unwrap_or(0);
                let addr = report.usb.address.unwrap_or(0);
                let pid = report.usb.product_id.unwrap_or(0);
                println!(
                    "[{}] USB Hardware     : 07ca:{pid:04x} (Bus {bus:03} Dev {addr:03}) {}",
                    if report.usb.can_open { "OK" } else { "!!" },
                    if report.usb.can_open {
                        "accessible non-root"
                    } else {
                        report.usb.open_error.as_deref().unwrap_or("access denied")
                    }
                );
            } else {
                println!("[--] USB Hardware     : AVerMedia GL310 (07ca:c835) not plugged in");
            }
            println!(
                "[{}] udev Rule        : /etc/udev/rules.d/70-avermedia-gl310.rules {}",
                if report.udev_rule_installed { "OK" } else { "!!" },
                if report.udev_rule_installed {
                    "installed"
                } else {
                    "missing -> run: sudo bash scripts/install_udev.sh"
                }
            );
            println!(
                "[{}] Firmware SHA-256 : {}",
                if report.firmware.valid { "OK" } else { "!!" },
                report.firmware.message
            );
            for t in &report.tools {
                println!(
                    "[{}] Tool {:<12}: {}",
                    if t.available { "OK" } else { "!!" },
                    t.name,
                    t.detail
                );
            }
            if !report.v4l2_devices.is_empty() {
                println!("     Existing V4L2    :");
                for d in &report.v4l2_devices {
                    println!("       - {d}");
                }
            }
            println!(
                "\nSummary: Phase 1 dry-test = {} | Full capture = {}",
                if report.ready_for_phase1 { "READY" } else { "NOT READY" },
                if report.ready_for_capture { "READY" } else { "NOT READY" }
            );
            Ok(())
        }

        Commands::Firmware { action } => match action {
            FirmwareCmd::Fetch { dest } => {
                eprintln!("[gl310-cli] Fetching and auditing official AVerMedia v3.2802.64.40 archive...");
                let (_fw, log) = fetch_and_install_official_firmware(dest.as_deref())?;
                println!("{log}");
                Ok(())
            }
            FirmwareCmd::Import { source, dest } => {
                let fw = import_firmware_from_path(&source, dest.as_deref())?;
                println!("Imported and verified GL310 firmware into {}", fw.directory.display());
                println!("  qpvidfwusb.bin : {}", fw.video_sha256);
                println!("  qpaudfwusb.bin : {}", fw.audio_sha256);
                Ok(())
            }
            FirmwareCmd::Verify { json } => {
                let st = inspect_firmware_status(fw_ov);
                if json {
                    println!("{}", serde_json::to_string_pretty(&st).map_err(|e| e.to_string())?);
                } else {
                    println!("{}", st.message);
                    if let Some(ref v) = st.video_sha256 {
                        println!("  qpvidfwusb.bin : {v}");
                    }
                    if let Some(ref a) = st.audio_sha256 {
                        println!("  qpaudfwusb.bin : {a}");
                    }
                }
                if st.valid {
                    Ok(())
                } else {
                    Err(st.message)
                }
            }
        },

        Commands::Info { json } => {
            let dev = Gl310Device::open().map_err(|e| e.to_string())?;
            let info = dev.read_info().map_err(|e| e.to_string())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?);
            } else {
                println!("AVerMedia GL310 (Bus {:03} Dev {:03})", info.bus, info.address);
                println!(
                    "  Chip revision : {:#010x} ({})",
                    info.chip_revision,
                    if info.chip_revision_ok { "OK" } else { "unexpected" }
                );
                println!("  MCU identity  : {}", info.mcu_identity);
                println!(
                    "  ARM Firmware  : {}",
                    if info.firmware_loaded { "loaded (QSOS)" } else { "not booted yet" }
                );
                println!("  Audio input   : {:?}", info.audio_source);
                if let Some(sig) = info.signal {
                    println!(
                        "  HDMI signal   : {}x{}{}{}Hz (HDCP={}, Audio={} {}Hz compressed={})",
                        sig.width,
                        sig.height,
                        if sig.interlaced { "i" } else { "p" },
                        sig.refresh_hz,
                        sig.hdcp,
                        sig.audio.present,
                        sig.audio.sample_rate_hz,
                        sig.audio.compressed_non_pcm
                    );
                } else {
                    println!("  HDMI signal   : No signal");
                }
            }
            Ok(())
        }

        Commands::Signal { json, watch } => {
            let dev = Gl310Device::open().map_err(|e| e.to_string())?;
            let running = setup_ctrlc();
            loop {
                let sig = dev.read_signal().map_err(|e| e.to_string())?;
                if json {
                    println!("{}", serde_json::to_string(&sig).map_err(|e| e.to_string())?);
                } else if let Some(s) = sig {
                    println!(
                        "HDMI LOCK: {}x{}{} @ {} Hz | total {}x{} | HDCP={} | Audio={} ({} Hz{})",
                        s.width,
                        s.height,
                        if s.interlaced { "i" } else { "p" },
                        s.refresh_hz,
                        s.h_total,
                        s.v_total,
                        s.hdcp,
                        if s.audio.present { "yes" } else { "no" },
                        s.audio.sample_rate_hz,
                        if s.audio.compressed_non_pcm {
                            ", COMPRESSED NON-PCM!"
                        } else {
                            ", LPCM"
                        }
                    );
                } else {
                    println!("NO HDMI SIGNAL");
                }
                if !watch || !running.load(Ordering::SeqCst) {
                    break;
                }
                sleep(Duration::from_millis(500));
            }
            Ok(())
        }

        Commands::DryTest { button_wait, json } => {
            let fw = discover_verified_firmware(fw_ov)?;
            let dev = Gl310Device::open().map_err(|e| e.to_string())?;
            let report = dev
                .run_dry_test(&fw, button_wait, |line| {
                    if !json {
                        println!("{line}");
                    }
                })
                .map_err(|e| e.to_string())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
            } else {
                println!("\n=== Phase 1 Dry-Test Result ===");
                println!(
                    "  USB 07ca:c835  : {}",
                    if report.usb_ok { "PASS" } else { "FAIL" }
                );
                println!(
                    "  Chip rev 0x38  : {:#010x} ({})",
                    report.chip_revision,
                    if report.chip_revision_ok { "PASS" } else { "WARN" }
                );
                println!("  MCU identity   : {}", report.mcu_identity);
                println!(
                    "  Safety 0x47/49 : {}",
                    if report.safety_guard_ok { "PASS (blocked)" } else { "FAIL" }
                );
                println!(
                    "  LED patterns   : {}",
                    if report.leds_cycled { "PASS" } else { "FAIL" }
                );
                if report.button_tested {
                    println!(
                        "  Button click   : {}",
                        if report.button_clicked { "PASS" } else { "NOT PRESSED" }
                    );
                }
                println!(
                    "  RAM Boot QSOS  : {}",
                    if report.qsos_verified { "PASS" } else { "FAIL" }
                );
                println!(
                    "  Mailbox Ping 0 : {}",
                    if report.mailbox_ping_ok { "PASS" } else { "FAIL" }
                );
            }
            if report.qsos_verified && report.mailbox_ping_ok && report.safety_guard_ok {
                Ok(())
            } else {
                Err("Dry-test failed one or more critical checks".into())
            }
        }

        Commands::Led { mode } => {
            let parsed = LedMode::from_label(&mode)?;
            let dev = Gl310Device::open().map_err(|e| e.to_string())?;
            let code = parsed.mcu_code().unwrap_or(0x04);
            dev.set_led_code(code).map_err(|e| e.to_string())?;
            println!("LED ring set to {mode} (MCU 0x2C = {code:#04x})");
            Ok(())
        }

        Commands::Button { exec } => {
            let dev = Gl310Device::open().map_err(|e| e.to_string())?;
            let running = setup_ctrlc();
            eprintln!("[gl310-cli] Watching physical GL310 button (Ctrl+C to exit)...");
            while running.load(Ordering::SeqCst) {
                match dev.poll_mcu_event(true) {
                    Ok(McuEvent::ButtonClick) => {
                        println!("CLICK");
                        if let Some(ref cmd) = exec {
                            let _ = ProcessCommand::new("sh").args(["-c", cmd]).status();
                        }
                    }
                    Ok(_) => {}
                    Err(e) => return Err(e.to_string()),
                }
                sleep(Duration::from_millis(120));
            }
            Ok(())
        }

        Commands::Capture {
            fps,
            bitrate,
            audio_bitrate,
            audio_in,
            analog_vol,
            audio_out,
        } => {
            let settings = Settings {
                mode: CaptureMode::H264Aac,
                fps_policy: fps.into(),
                video_kbps: bitrate,
                audio_kbps: audio_bitrate,
                audio_input: audio_in.into(),
                analog_volume: analog_vol,
                audio_out_jack: audio_out,
                ..Default::default()
            };
            settings.validate()?;

            let dev = open_and_ensure_booted(fw_ov)?;
            let sig = dev.read_signal().ok().flatten();
            let regs = dev
                .start_encoder(&settings, sig.as_ref())
                .map_err(|e| e.to_string())?;
            eprintln!(
                "[gl310-cli] Capturing H.264+AAC MPEG-TS -> stdout ({}x{}@{} fps, {} kbps)",
                regs.out_width, regs.out_height, regs.out_fps, settings.video_kbps
            );

            let running = setup_ctrlc();
            let mut deframer = TsDeframer::new();
            let mut stdout = io::stdout().lock();

            let mut stream_err: Option<String> = None;
            while running.load(Ordering::SeqCst) {
                match dev.poll_stream_chunk(CaptureMode::H264Aac) {
                    Ok(Some(chunk)) => {
                        let (aligned, _) = deframer.push(&chunk.data);
                        if !aligned.is_empty() && stdout.write_all(&aligned).is_err() {
                            break;
                        }
                    }
                    Ok(None) => sleep(Duration::from_millis(2)),
                    Err(e) => {
                        stream_err = Some(format!("USB stream error: {e}"));
                        break;
                    }
                }
            }
            let _ = dev.stop_encoder();
            let stats = deframer.stats();
            eprintln!(
                "[gl310-cli] Stopped. Packets={} ContinuityErrors={} Keyframes={}",
                stats.packets, stats.continuity_errors, stats.keyframes
            );
            if let Some(err) = stream_err {
                return Err(err);
            }
            Ok(())
        }

        Commands::Record {
            output,
            fps,
            bitrate,
            audio_bitrate,
            audio_in,
            analog_vol,
            audio_out,
            duration,
        } => {
            let settings = Settings {
                mode: CaptureMode::H264Aac,
                fps_policy: fps.into(),
                video_kbps: bitrate,
                audio_kbps: audio_bitrate,
                audio_input: audio_in.into(),
                analog_volume: analog_vol,
                audio_out_jack: audio_out,
                ..Default::default()
            };
            settings.validate()?;

            let dev = open_and_ensure_booted(fw_ov)?;
            let sig = dev.read_signal().ok().flatten();
            let regs = dev
                .start_encoder(&settings, sig.as_ref())
                .map_err(|e| e.to_string())?;
            let hub = StreamHub::new();
            let recorder = TsRecorder::start(&hub, output)?;
            eprintln!(
                "[gl310-cli] Recording {}x{}@{} fps ({} kbps) — Ctrl+C to stop",
                regs.out_width, regs.out_height, regs.out_fps, settings.video_kbps
            );

            let running = setup_ctrlc();
            let started = Instant::now();
            let mut deframer = TsDeframer::new();
            let mut stream_err: Option<String> = None;
            while running.load(Ordering::SeqCst) {
                if let Some(secs) = duration {
                    if started.elapsed() >= Duration::from_secs(secs) {
                        break;
                    }
                }
                match dev.poll_stream_chunk(CaptureMode::H264Aac) {
                    Ok(Some(chunk)) => {
                        let (aligned, idr) = deframer.push(&chunk.data);
                        let hdr = deframer.bootstrap_headers();
                        hub.publish_ts(aligned, idr, &hdr, deframer.stats());
                    }
                    Ok(None) => sleep(Duration::from_millis(2)),
                    Err(e) => {
                        stream_err = Some(format!("USB stream error: {e}"));
                        break;
                    }
                }
            }
            let _ = dev.stop_encoder();
            let summary = recorder.stop();
            println!(
                "Saved {} ({:.1}s, {} bytes) — {}",
                summary.final_path, summary.duration_secs, summary.bytes, summary.note
            );
            if let Some(err) = stream_err {
                return Err(err);
            }
            Ok(())
        }

        Commands::V4l2 {
            device,
            force_1080p30,
            mjpeg,
            res,
            fps,
            quality,
        } => {
            let settings = Settings {
                mode: if mjpeg {
                    CaptureMode::Mjpeg
                } else {
                    CaptureMode::H264Aac
                },
                v4l2_device: device.clone(),
                v4l2_format: if force_1080p30 {
                    V4l2FormatPolicy::Fixed1080p30
                } else {
                    V4l2FormatPolicy::Auto
                },
                fps_policy: if force_1080p30 {
                    FpsPolicy::Force30
                } else {
                    FpsPolicy::Auto
                },
                mjpeg_resolution: MjpegResolution::from_label(&res)?,
                mjpeg_fps: fps,
                mjpeg_quality: quality,
                ..Default::default()
            };
            settings.validate()?;

            let dev = open_and_ensure_booted(fw_ov)?;
            let sig = dev.read_signal().ok().flatten();
            let regs = dev
                .start_encoder(&settings, sig.as_ref())
                .map_err(|e| e.to_string())?;
            let hub = StreamHub::new();
            let v4l2_out = V4l2Output::start(&hub, settings.mode, device.clone(), settings.v4l2_format)?;
            eprintln!(
                "[gl310-cli] V4L2 active on {device} ({:?} {}x{}@{} fps) — Ctrl+C to stop",
                settings.mode, regs.out_width, regs.out_height, regs.out_fps
            );

            let running = setup_ctrlc();
            let mut ts_d = TsDeframer::new();
            let mut mj_d = MjpegDeframer::new();
            let mut stream_err: Option<String> = None;
            while running.load(Ordering::SeqCst) {
                match dev.poll_stream_chunk(settings.mode) {
                    Ok(Some(chunk)) => match settings.mode {
                        CaptureMode::H264Aac => {
                            let (aligned, idr) = ts_d.push(&chunk.data);
                            let hdr = ts_d.bootstrap_headers();
                            hub.publish_ts(aligned, idr, &hdr, ts_d.stats());
                        }
                        CaptureMode::Mjpeg => {
                            for frame in mj_d.push(&chunk.data) {
                                hub.publish_jpeg(frame);
                            }
                        }
                    },
                    Ok(None) => sleep(Duration::from_millis(2)),
                    Err(e) => {
                        stream_err = Some(format!("USB stream error: {e}"));
                        break;
                    }
                }
            }
            v4l2_out.stop();
            let _ = dev.stop_encoder();
            if let Some(err) = stream_err {
                return Err(err);
            }
            Ok(())
        }

        Commands::Obs {
            device,
            force_1080p30,
            bitrate,
            audio_bitrate,
            audio_in,
            analog_vol,
            audio_out,
            srt_port,
            http_port,
            lan,
            monitor,
        } => {
            let settings = Settings {
                mode: CaptureMode::H264Aac,
                v4l2_device: device.clone(),
                v4l2_format: if force_1080p30 {
                    V4l2FormatPolicy::Fixed1080p30
                } else {
                    V4l2FormatPolicy::Auto
                },
                fps_policy: if force_1080p30 {
                    FpsPolicy::Force30
                } else {
                    FpsPolicy::Auto
                },
                video_kbps: bitrate,
                audio_kbps: audio_bitrate,
                audio_input: audio_in.into(),
                analog_volume: analog_vol,
                audio_out_jack: audio_out,
                srt_port,
                http_port,
                expose_lan: lan,
                pc_monitor: monitor,
                ..Default::default()
            };
            settings.validate()?;

            let dev = open_and_ensure_booted(fw_ov)?;
            let sig = dev.read_signal().ok().flatten();
            let regs = dev
                .start_encoder(&settings, sig.as_ref())
                .map_err(|e| e.to_string())?;
            let hub = StreamHub::new();
            let vu = VuMeter::new();
            let mon = MonitorControl::from_settings(&settings);
            let outputs = ActiveOutputs::start_all(&settings, &hub, vu, mon, true);
            for w in outputs.warnings.lock().unwrap().iter() {
                eprintln!("[gl310-cli] WARN: {w}");
            }
            let disp_host = display_host_for(settings.expose_lan);
            println!("=== GL310 OBS Studio Bridge Running ===");
            println!(
                "  Encoder      : H.264+AAC {}x{}@{} fps ({} kbps)",
                regs.out_width, regs.out_height, regs.out_fps, settings.video_kbps
            );
            println!("  V4L2 Camera  : {device} (AVerMedia GL310)");
            println!("  PipeWire Mic : GL310 Audio");
            println!("  SRT Source   : srt://{disp_host}:{srt_port}?mode=caller&transtype=live&latency=120000");
            println!("  HTTP TS      : http://{disp_host}:{http_port}/stream.ts");
            println!("  HTTP MJPEG   : http://{disp_host}:{http_port}/mjpeg");
            println!("  OBS Tip      : probesize=500000 analyzeduration=500000");
            println!("Press Ctrl+C to stop cleanly.");

            let running = setup_ctrlc();
            let mut ts_d = TsDeframer::new();
            let mut stream_err: Option<String> = None;
            while running.load(Ordering::SeqCst) {
                match dev.poll_stream_chunk(CaptureMode::H264Aac) {
                    Ok(Some(chunk)) => {
                        let (aligned, idr) = ts_d.push(&chunk.data);
                        let hdr = ts_d.bootstrap_headers();
                        hub.publish_ts(aligned, idr, &hdr, ts_d.stats());
                    }
                    Ok(None) => sleep(Duration::from_millis(2)),
                    Err(e) => {
                        stream_err = Some(format!("USB stream error: {e}"));
                        break;
                    }
                }
            }
            drop(outputs);
            let _ = dev.stop_encoder();
            if let Some(err) = stream_err {
                return Err(err);
            }
            Ok(())
        }

        Commands::Kvm {
            res,
            fps,
            quality,
            http,
            v4l2,
        } => {
            let (host, port) = parse_host_port(&http)?;
            let settings = Settings {
                mode: CaptureMode::Mjpeg,
                mjpeg_resolution: MjpegResolution::from_label(&res)?,
                mjpeg_fps: fps,
                mjpeg_quality: quality,
                http_port: port,
                expose_lan: host == "0.0.0.0",
                ..Default::default()
            };
            settings.validate()?;

            let hub = StreamHub::new();
            let _http_srv = HttpServer::start(hub.clone(), &host, port)?;
            let _v4l2_out = if let Some(ref dev_path) = v4l2 {
                Some(V4l2Output::start(
                    &hub,
                    CaptureMode::Mjpeg,
                    dev_path.clone(),
                    V4l2FormatPolicy::Auto,
                )?)
            } else {
                None
            };

            let disp_host = if host == "0.0.0.0" {
                display_host_for(true)
            } else {
                host.clone()
            };
            println!("=== GL310 Raspberry Pi KVM Daemon (Hardware MJPEG, Zero-CPU Decode) ===");
            println!("  Web Viewer   : http://{disp_host}:{port}/");
            println!("  MJPEG Stream : http://{disp_host}:{port}/mjpeg");
            println!("  Snapshot     : http://{disp_host}:{port}/snapshot.jpg");
            if let Some(ref dev_path) = v4l2 {
                println!("  V4L2 MJPEG   : {dev_path} (PiKVM / uStreamer ready)");
            }
            println!("  Target Mode  : {res} @ {fps} fps, quality {quality}/15 (auto-restarts on BIOS->OS mode switch)");

            let running = setup_ctrlc();
            let dev = open_and_ensure_booted(fw_ov)?;
            let mut cur_sig = dev.read_signal().ok().flatten();
            let mut regs = dev
                .start_encoder(&settings, cur_sig.as_ref())
                .map_err(|e| e.to_string())?;
            eprintln!(
                "[gl310-kvm] Hardware MJPEG started: {}x{}@{} fps",
                regs.out_width, regs.out_height, regs.out_fps
            );

            let mut deframer = MjpegDeframer::new();
            let mut last_sig_check = Instant::now();
            let mut stream_err: Option<String> = None;

            while running.load(Ordering::SeqCst) {
                match dev.poll_stream_chunk(CaptureMode::Mjpeg) {
                    Ok(Some(chunk)) => {
                        for frame in deframer.push(&chunk.data) {
                            hub.publish_jpeg(frame);
                        }
                    }
                    Ok(None) => sleep(Duration::from_millis(2)),
                    Err(e) => {
                        stream_err = Some(format!("KVM USB read error: {e}"));
                        break;
                    }
                }

                // Auto-recover when BIOS / UEFI / Linux changes HDMI resolution!
                if last_sig_check.elapsed() >= Duration::from_millis(800) {
                    last_sig_check = Instant::now();
                    if let Ok(new_sig) = dev.read_signal() {
                        let changed = match (cur_sig, new_sig) {
                            (Some(a), Some(b)) => {
                                a.width != b.width
                                    || a.height != b.height
                                    || a.refresh_hz != b.refresh_hz
                                    || a.interlaced != b.interlaced
                            }
                            (None, Some(_)) => true,
                            _ => false,
                        };
                        cur_sig = new_sig;
                        if changed {
                            if let Some(ref ns) = new_sig {
                                eprintln!(
                                    "[gl310-kvm] HDMI resolution changed -> {}x{}@{}Hz — reconfiguring MJPEG...",
                                    ns.width, ns.height, ns.refresh_hz
                                );
                                hub.clear_snapshot();
                                let _ = dev.stop_encoder();
                                deframer = MjpegDeframer::new();
                                match dev.start_encoder(&settings, Some(ns)) {
                                    Ok(r) => {
                                        regs = r;
                                        eprintln!(
                                            "[gl310-kvm] Resumed MJPEG: {}x{}@{} fps",
                                            regs.out_width, regs.out_height, regs.out_fps
                                        );
                                    }
                                    Err(e) => {
                                        stream_err = Some(format!(
                                            "Failed reconfiguring MJPEG after HDMI change: {e}"
                                        ));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let _ = dev.stop_encoder();
            if let Some(err) = stream_err {
                // Return non-zero exit status so systemd `Restart=on-failure` recovers automatically.
                return Err(err);
            }
            Ok(())
        }

        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "gl310-cli", &mut io::stdout());
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_handles_valid_and_invalid_inputs() {
        assert_eq!(parse_host_port("0.0.0.0:4848").unwrap(), ("0.0.0.0".into(), 4848));
        assert_eq!(parse_host_port("127.0.0.1:8080").unwrap(), ("127.0.0.1".into(), 8080));
        assert!(parse_host_port("localhost").is_err());
        assert!(parse_host_port("127.0.0.1:notaport").is_err());
    }

    #[test]
    fn clap_cli_parses_all_core_subcommands() {
        assert!(Cli::try_parse_from(["gl310-cli", "doctor", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "firmware", "verify"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "dry-test", "--button-wait", "0"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "capture", "--fps", "30", "--audio-in", "jack", "--audio-out"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "capture", "--audio-out", "false"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "record", "out.mp4", "--audio-out"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "v4l2", "--force-1080p30"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "obs", "--lan", "--monitor", "--audio-out"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "kvm", "--res", "720p", "--fps", "15", "--quality", "12"]).is_ok());
        assert!(Cli::try_parse_from(["gl310-cli", "completions", "bash"]).is_ok());
    }
}
