//! Hardware protocol constants, register encoders, full IT6604 HDMI timing table
//! (verified via static REA analysis of AVer330USB.sys `InterfaceNUC100::getHdmiVideo_6604`),
//! I2C board initialization tables, and hard safety guards for the AVerMedia GL310 (`07ca:c835`).

use serde::{Deserialize, Serialize};

use crate::settings::{AudioInput, CaptureMode, FpsPolicy, Settings};

// ---------------------------------------------------------------------------
// USB Identity & Bulk Endpoints
// ---------------------------------------------------------------------------

pub const USB_VENDOR_ID: u16 = 0x07CA;
pub const USB_PRODUCT_ID: u16 = 0xC835;
/// Locked/recovery PID observed after malformed boot sequences; requires unplugging 10 s.
pub const USB_PRODUCT_ID_LOCKED: u16 = 0xD835;

pub const EP_CMD_OUT: u8 = 0x04;
pub const EP_CMD_IN: u8 = 0x83;
pub const EP_DATA_OUT: u8 = 0x02;
pub const EP_DATA_IN: u8 = 0x81;

/// Maximum words per DMA transfer block (8192 words = 32 KiB).
pub const DMA_BLOCK_WORDS: u32 = 0x2000;

// ---------------------------------------------------------------------------
// QL202 SoC & ARM Mailbox Registers
// ---------------------------------------------------------------------------

pub const REG_GPIO_DATA: u32 = 0x0000;
pub const REG_CHIP_REV: u32 = 0x0038;
pub const EXPECTED_CHIP_REV: u32 = 0x0001_0020;
pub const REG_SYS_CTRL_50: u32 = 0x0050;

/// QL202 GPIO controller registers (verified in `AVer330USB.sys::CQLCodec_GetGPIOBitValue`).
pub const REG_GPIO_DIR: u32 = 0x0610; // 0 = input, 1 = output
pub const REG_GPIO_OUT: u32 = 0x0614; // output latch value
pub const REG_GPIO_IN: u32 = 0x0618; // input pin levels

pub const REG_IRQ_STATUS: u32 = 0x0800;
/// Bit 16 of 0x0800 indicates a pending ARM->Host mailbox message.
pub const IRQ_ARM_MAILBOX_BIT: u32 = 1 << 16;
/// Bit 12 of `REG_GPIO_IN` (`0x0618`) is the active-low MCU interrupt line (ProjectC875 GPIO 12).
/// Verified in `AVer330USB.sys` (`0x428cf` pin = `0x0c`, `0x47d64` active-low `== 0`).
pub const GPIO_MCU_INT_BIT: u32 = 1 << 12;

pub const REG_TO_ARM_MSG: u32 = 0x06FC;
pub const REG_TO_ARM_PARAM0: u32 = 0x06F8;
pub const REG_TO_ARM_PARAM1: u32 = 0x06F4;
pub const REG_TO_ARM_PARAM2: u32 = 0x06F0;
pub const REG_TO_ARM_PARAM3: u32 = 0x06EC;
pub const REG_TO_ARM_PARAM4: u32 = 0x06E8;
pub const REG_TO_ARM_PARAM5: u32 = 0x06E4;
pub const REG_TO_ARM_PARAM6: u32 = 0x06E0;
pub const REG_TO_ARM_PARAM7: u32 = 0x06DC;
pub const REG_TO_ARM_PARAM8: u32 = 0x06D8;
pub const REG_TO_ARM_AUDIO_CTRL: u32 = 0x06D4;
pub const REG_TO_ARM_AUDIO_EX: u32 = 0x06D0;
pub const REG_TO_ARM_STATUS: u32 = 0x06CC;

pub const REG_FROM_ARM_STATUS: u32 = 0x06C8;
pub const REG_FROM_ARM_MSG: u32 = 0x06B0;

// Mailbox opcodes (Host -> ARM)
pub const MBOX_PING: u32 = 0x00;
pub const MBOX_START_ENCODER: u32 = 0x01;
pub const MBOX_STOP_ENCODER: u32 = 0x02;
pub const MBOX_SET_AUDIO_INPUT_VOL: u32 = 0x0A;
pub const MBOX_SET_EXT_CONFIG: u32 = 0x10;
pub const MBOX_COMPLETE_BUFFER: u32 = 0x30;
pub const MBOX_ACK_LOW: u32 = 0x31;
pub const MBOX_ACK_HIGH: u32 = 0xA2;
pub const MBOX_SYSTEM_OPEN: u32 = 0xF1;
pub const MBOX_SYSTEM_LINK: u32 = 0xF2;
pub const MBOX_SYSTEM_CLOSE: u32 = 0xF3;

// Mailbox message kinds (ARM -> Host)
pub const MSG_KIND_FRAME: u32 = 0x40;
/// MPEG-TS multiplexed H.264 + AAC buffer tag (`p0 & 0xFFFF`).
pub const STREAM_TAG_TS: u32 = 0x0083;
/// Elementary video / MJPEG frame buffer tag (`p0 & 0xFFFF`).
pub const STREAM_TAG_VIDEO: u32 = 0x0081;

// ARM RAM memory map (word addresses)
pub const ARM_VIDEO_FW_WORD_BASE: u32 = 0x0000_0000;
pub const ARM_AUDIO_FW_WORD_BASE: u32 = 0x0004_0000;
pub const ARM_AUDIO_BSS_CLEAR_CHUNKS: [u32; 3] = [0x2000, 0x2000, 0x1E3B];
pub const ARM_CFG_INT_MODE_WORD: u32 = 0x002F_1090 / 4;
pub const ARM_CFG_FIXED_MODE_WORD: u32 = 0x002F_1094 / 4;
pub const ARM_CFG_BASE_WORD: u32 = 0x002F_2000 / 4;
pub const ARM_QSOS_MAGIC_WORD: u32 = 0x0040;
pub const ARM_QSOS_MAGIC: &[u8; 4] = b"QSOS";

// ---------------------------------------------------------------------------
// NUC100 MCU Registers (7-bit I2C addr 0x15 = 0x2B >> 1) & Hard Safety Guard
// ---------------------------------------------------------------------------

pub const MCU_I2C_ADDR: u8 = 0x15;
pub const MCU_REG_IDENTITY: u8 = 0x0A; // ASCII model string "G310" at offsets 0x0A..0x0D
/// MCU busy / signal-transition status (`0x00` == stable; polled before reading `0x1C`/`0x1D`/`0x24`/`0x25`).
pub const MCU_REG_BUSY_STATUS: u8 = 0x1B;
/// Bit 2 (`0x04`): HDMI video lock (`getHdmiVideoStatus_6604`); Bit 4 (`0x10`): HDMI audio detected (`getHdmiAudioStatus_6604`).
pub const MCU_REG_SIGNAL_STATUS: u8 = 0x1C;
pub const MCU_REG_TIMING_BLOCK: u8 = 0x1D; // 7 bytes: Htotal, Hactive, Vtotal, Vactive, pclk div
/// HDMI audio sample-rate register (`getHdmiAudio_6604`, `0x32345`): low nibble 0=44.1kHz, 2=48kHz, 3=32kHz.
pub const MCU_REG_AUDIO_RATE: u8 = 0x24;
/// HDCP protection status (`getHdmiVideo_HDCP`, `0x31126`): bit 0 (`0x01`) == HDCP active.
pub const MCU_REG_HDCP_STATUS: u8 = 0x25;
pub const MCU_REG_SYNC_POLARITY: u8 = 0x26; // Bit 0/1: HSync/VSync polarity
pub const MCU_REG_EVENT: u8 = 0x2B; // Read event code; write 0x00 to clear click
pub const MCU_REG_LED: u8 = 0x2C; // 0x03 fade, 0x04 clockwise, 0x05 solid, 0x0B fast clockwise
pub const MCU_REG_AUDIO_SOURCE: u8 = 0x2E; // 0x04 = HDMI IN, 0x05 = Jack 3.5 mm AUDIO IN
pub const MCU_REG_RTC: u8 = 0x30; // 6-byte BCD YY MM DD hh mm ss
pub const MCU_REG_VIDEO_SOURCE: u8 = 0x3A; // 0x02 = HDMI video
pub const MCU_REG_WAKE: u8 = 0x40; // Write 0x01 to wake MCU
/// Analog Jack 3.5 mm line-in gain (`0x80 | volume`, where volume in 1..=11), verified via REA.
pub const MCU_REG_ANALOG_VOL: u8 = 0x41;

/// Hardcoded forbidden MCU registers: writing these alters non-volatile MCU profile / HDCP flash.
/// Every low-level MCU write path MUST reject these addresses unconditionally.
pub const FORBIDDEN_PERMANENT_MCU_REGS: [u8; 2] = [0x47, 0x49];

pub const MCU_SIGNAL_HDMI_LOCK: u8 = 0x04;
pub const MCU_SIGNAL_HDMI_AUDIO: u8 = 0x10;
/// Bit 0 of MCU register `0x25` (`MCU_REG_HDCP_STATUS`), verified in `getHdmiVideo_HDCP` (`0x31186`).
pub const MCU_HDCP_ACTIVE_BIT: u8 = 0x01;

/// Check whether a raw USB command attempts a forbidden permanent MCU write.
pub fn check_command_safety(cmd: &[u8]) -> Result<(), String> {
    if cmd.first().copied() == Some(0x0B) && cmd.len() > 8 {
        let reg = cmd[8];
        if FORBIDDEN_PERMANENT_MCU_REGS.contains(&reg) {
            return Err(format!(
                "SAFETY GUARD: refused permanent MCU write to register {reg:#04x} (0x47/0x49 are permanently blocked)"
            ));
        }
    }
    Ok(())
}

/// Check whether an MCU register address is safe for volatile RAM writes.
pub fn check_mcu_write_safety(reg: u8) -> Result<(), String> {
    if FORBIDDEN_PERMANENT_MCU_REGS.contains(&reg) {
        return Err(format!(
            "SAFETY GUARD: refused permanent MCU write to register {reg:#04x} (0x47/0x49 are permanently blocked)"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MCU Events (ProjectC875::CMCUEventMgr via MCU reg 0x2B)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McuEvent {
    None,
    HdmiSignalChange(u8),
    AudioSourceToggle,
    ButtonClick,
    Unknown(u8),
}

impl McuEvent {
    pub fn from_raw(code: u8) -> Self {
        match code {
            0 => McuEvent::None,
            1 | 7 | 10 => McuEvent::HdmiSignalChange(code),
            4 => McuEvent::AudioSourceToggle,
            6 => McuEvent::ButtonClick,
            other => McuEvent::Unknown(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Audio sample-rate decoding (MCU reg 0x24 + reg 0x1C)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSignalInfo {
    pub present: bool,
    pub sample_rate_hz: u32,
    pub compressed_non_pcm: bool,
    pub raw_status: u8,
}

impl AudioSignalInfo {
    /// Decode MCU audio sample-rate register (`0x24`) and signal status byte (`0x1C`).
    /// Verified directly in `AVer330USB.sys::InterfaceNUC100::getHdmiAudio_6604` (`0x32345..0x323d5`)
    /// and `getHdmiAudioStatus_6604` (`0x3271f..0x32787`):
    /// - `reg_1c & 0x10` indicates HDMI audio detected.
    /// - Low nibble of `reg_24` encodes IEC-60958 sampling frequency:
    ///   `0x00` = 44.1 kHz, `0x02` = 48 kHz, `0x03` = 32 kHz, `0x08` = 88.2 kHz, `0x0A` = 96 kHz.
    /// - When HDMI audio is detected but `reg_24` reports bit 6 (`0x40`) or an unrecognized rate nibble,
    ///   flag `compressed_non_pcm` so the user is warned to switch console audio to Stereo LPCM.
    pub fn from_mcu_bytes(reg_24: u8, reg_1c: u8) -> Self {
        let present = (reg_1c & MCU_SIGNAL_HDMI_AUDIO) != 0;
        let (sample_rate_hz, known_rate) = match reg_24 & 0x0F {
            0x00 => (44_100, true),
            0x02 => (48_000, true),
            0x03 => (32_000, true),
            0x08 => (88_200, true),
            0x0A => (96_000, true),
            _ => (48_000, false),
        };
        let compressed_non_pcm = present && (((reg_24 & 0x40) != 0) || !known_rate);
        Self {
            present,
            sample_rate_hz,
            compressed_non_pcm,
            raw_status: reg_24,
        }
    }

    /// QL202 `EncAudioControlParam` (`0x6D4`) bits 12..15 sample-rate selector.
    /// Verified directly in `AVer330USB.sys` (`0x39fbb..0x3a025`):
    ///   `0` = 44.1 kHz (`0x0000`), `1` = 48 kHz (`0x1000`), `2` = 32 kHz (`0x2000`).
    pub fn encoder_rate_code(self) -> u32 {
        match self.sample_rate_hz {
            44_100 => 0,
            32_000 => 2,
            _ => 1, // 48 kHz default
        }
    }
}

// ---------------------------------------------------------------------------
// Complete 49-entry HDMI / VESA Mode Table from `getHdmiVideo_6604` (0xfa0c0)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HdmiModeEntry {
    pub h_active: u16,
    pub v_active: u16,
    pub h_total: u16,
    pub v_total: u16,
    pub pclk_khz: u32,
    pub fps: u16,
    pub interlaced: bool,
    pub out_width: u16,
    pub out_height: u16,
    /// `EncInputControl` (register `0x6F0`) base value from the official driver table.
    pub enc_input_ctrl: u32,
    /// `EncSyncMode` value from the official driver table.
    pub enc_sync_mode: u32,
}

impl HdmiModeEntry {
    pub const fn enc_picture_resolution(&self) -> u32 {
        ((self.out_height as u32) << 16) | (self.out_width as u32)
    }
}

/// Complete 49-entry HDMI/VESA timing table verified directly from `AVer330USB.sys`
/// at VMA `0xfa0c0` (`InterfaceNUC100::getHdmiVideo_6604`).
/// Covers CEA HD/SD (1080p24/25/30/50/60, 1080i50/60, 720p50/60, 480p/576p/480i/576i/240p)
/// and VESA PC/BIOS/UEFI resolutions (1680x1050, 1440x900, 1360x768, 1280x1024, 1280x800,
/// 1280x768, 1024x768, 800x600, 640x480).
pub const HDMI_MODES: [HdmiModeEntry; 49] = [
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2750, v_total: 1125, pclk_khz: 74_000, fps: 24, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0C58_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2640, v_total: 1125, pclk_khz: 74_000, fps: 25, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0CD9_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2200, v_total: 1125, pclk_khz: 74_000, fps: 30, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0F5E_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2640, v_total: 1125, pclk_khz: 148_000, fps: 50, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0CF2_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2200, v_total: 1125, pclk_khz: 148_500, fps: 60, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0F7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 540, h_total: 2640, v_total: 562, pclk_khz: 74_000, fps: 50, interlaced: true, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0CD9_0608, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 540, h_total: 2200, v_total: 563, pclk_khz: 74_000, fps: 60, interlaced: true, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0F5E_0608, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1280, v_active: 720, h_total: 1650, v_total: 750, pclk_khz: 74_000, fps: 60, interlaced: false, out_width: 1280, out_height: 720, enc_input_ctrl: 0x3E7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1280, v_active: 720, h_total: 1980, v_total: 750, pclk_khz: 74_000, fps: 50, interlaced: false, out_width: 1280, out_height: 720, enc_input_ctrl: 0x3972_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 720, v_active: 480, h_total: 858, v_total: 525, pclk_khz: 27_000, fps: 60, interlaced: false, out_width: 720, out_height: 480, enc_input_ctrl: 0x3E7C_0789, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 720, v_active: 576, h_total: 864, v_total: 625, pclk_khz: 27_000, fps: 50, interlaced: false, out_width: 720, out_height: 576, enc_input_ctrl: 0x3972_0789, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 720, v_active: 240, h_total: 858, v_total: 262, pclk_khz: 13_500, fps: 60, interlaced: true, out_width: 720, out_height: 480, enc_input_ctrl: 0x2F5E_0788, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 1440, v_active: 240, h_total: 1716, v_total: 262, pclk_khz: 27_000, fps: 60, interlaced: true, out_width: 720, out_height: 480, enc_input_ctrl: 0x2F5E_0788, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 720, v_active: 288, h_total: 864, v_total: 312, pclk_khz: 13_500, fps: 50, interlaced: true, out_width: 720, out_height: 576, enc_input_ctrl: 0x2CD9_0788, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 1440, v_active: 288, h_total: 1728, v_total: 312, pclk_khz: 27_000, fps: 50, interlaced: true, out_width: 720, out_height: 576, enc_input_ctrl: 0x2CD9_0788, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 720, v_active: 240, h_total: 858, v_total: 262, pclk_khz: 13_500, fps: 60, interlaced: false, out_width: 720, out_height: 240, enc_input_ctrl: 0x2F7C_0789, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2080, v_total: 1111, pclk_khz: 138_200, fps: 60, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0F7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1920, v_active: 1080, h_total: 2080, v_total: 1111, pclk_khz: 138_200, fps: 60, interlaced: false, out_width: 1920, out_height: 1080, enc_input_ctrl: 0x0F7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1680, v_active: 1050, h_total: 2240, v_total: 1089, pclk_khz: 146_250, fps: 60, interlaced: false, out_width: 1680, out_height: 1050, enc_input_ctrl: 0x0F7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1680, v_active: 1050, h_total: 2256, v_total: 1087, pclk_khz: 147_136, fps: 60, interlaced: false, out_width: 1680, out_height: 1050, enc_input_ctrl: 0x0F7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1680, v_active: 1050, h_total: 1840, v_total: 1080, pclk_khz: 119_230, fps: 60, interlaced: false, out_width: 1680, out_height: 1050, enc_input_ctrl: 0x0F7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1440, v_active: 900, h_total: 1904, v_total: 934, pclk_khz: 106_500, fps: 60, interlaced: false, out_width: 1440, out_height: 900, enc_input_ctrl: 0x0F7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1440, v_active: 900, h_total: 1895, v_total: 934, pclk_khz: 106_500, fps: 60, interlaced: false, out_width: 1440, out_height: 900, enc_input_ctrl: 0x0F7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1440, v_active: 900, h_total: 1600, v_total: 926, pclk_khz: 88_890, fps: 60, interlaced: false, out_width: 1440, out_height: 900, enc_input_ctrl: 0x0F7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1360, v_active: 768, h_total: 1792, v_total: 795, pclk_khz: 85_500, fps: 60, interlaced: false, out_width: 1360, out_height: 768, enc_input_ctrl: 0x197C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1360, v_active: 768, h_total: 1776, v_total: 798, pclk_khz: 86_400, fps: 60, interlaced: false, out_width: 1360, out_height: 768, enc_input_ctrl: 0x197C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1360, v_active: 768, h_total: 1776, v_total: 795, pclk_khz: 84_200, fps: 60, interlaced: false, out_width: 1360, out_height: 768, enc_input_ctrl: 0x197C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1360, v_active: 768, h_total: 1520, v_total: 790, pclk_khz: 72_000, fps: 60, interlaced: false, out_width: 1360, out_height: 768, enc_input_ctrl: 0x197C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1280, v_active: 1024, h_total: 1712, v_total: 1060, pclk_khz: 108_000, fps: 60, interlaced: false, out_width: 1280, out_height: 1024, enc_input_ctrl: 0x0F7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 1024, h_total: 1688, v_total: 1066, pclk_khz: 108_000, fps: 60, interlaced: false, out_width: 1280, out_height: 1024, enc_input_ctrl: 0x0F7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1280, v_active: 1024, h_total: 1680, v_total: 1066, pclk_khz: 108_000, fps: 60, interlaced: false, out_width: 1280, out_height: 1024, enc_input_ctrl: 0x0F7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 1280, v_active: 1024, h_total: 1440, v_total: 1054, pclk_khz: 90_900, fps: 60, interlaced: false, out_width: 1280, out_height: 1024, enc_input_ctrl: 0x0F7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1280, v_active: 800, h_total: 1680, v_total: 828, pclk_khz: 84_300, fps: 60, interlaced: false, out_width: 1280, out_height: 800, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 800, h_total: 1688, v_total: 831, pclk_khz: 84_300, fps: 60, interlaced: false, out_width: 1280, out_height: 800, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 800, h_total: 1440, v_total: 823, pclk_khz: 71_000, fps: 60, interlaced: false, out_width: 1280, out_height: 800, enc_input_ctrl: 0x1E7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1280, v_active: 768, h_total: 1680, v_total: 795, pclk_khz: 80_300, fps: 60, interlaced: false, out_width: 1280, out_height: 768, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 768, h_total: 1688, v_total: 798, pclk_khz: 80_300, fps: 60, interlaced: false, out_width: 1280, out_height: 768, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 768, h_total: 1664, v_total: 795, pclk_khz: 79_000, fps: 60, interlaced: false, out_width: 1280, out_height: 768, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1280, v_active: 768, h_total: 1440, v_total: 790, pclk_khz: 67_700, fps: 60, interlaced: false, out_width: 1280, out_height: 768, enc_input_ctrl: 0x1E7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 1024, v_active: 768, h_total: 1344, v_total: 795, pclk_khz: 64_000, fps: 60, interlaced: false, out_width: 1024, out_height: 768, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1024, v_active: 768, h_total: 1344, v_total: 806, pclk_khz: 65_000, fps: 60, interlaced: false, out_width: 1024, out_height: 768, enc_input_ctrl: 0x1E7C_0789, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 1024, v_active: 768, h_total: 1328, v_total: 798, pclk_khz: 62_900, fps: 60, interlaced: false, out_width: 1024, out_height: 768, enc_input_ctrl: 0x1E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 1024, v_active: 768, h_total: 1184, v_total: 790, pclk_khz: 56_000, fps: 60, interlaced: false, out_width: 1024, out_height: 768, enc_input_ctrl: 0x1E7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 800, v_active: 600, h_total: 1056, v_total: 628, pclk_khz: 40_000, fps: 60, interlaced: false, out_width: 800, out_height: 600, enc_input_ctrl: 0x3E7C_0609, enc_sync_mode: 0x11 },
    HdmiModeEntry { h_active: 800, v_active: 600, h_total: 1024, v_total: 624, pclk_khz: 38_400, fps: 60, interlaced: false, out_width: 800, out_height: 600, enc_input_ctrl: 0x3E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 800, v_active: 600, h_total: 960, v_total: 618, pclk_khz: 35_600, fps: 60, interlaced: false, out_width: 800, out_height: 600, enc_input_ctrl: 0x3E7C_0709, enc_sync_mode: 0x0F },
    HdmiModeEntry { h_active: 640, v_active: 480, h_total: 800, v_total: 497, pclk_khz: 24_000, fps: 60, interlaced: false, out_width: 640, out_height: 480, enc_input_ctrl: 0x3E7C_0689, enc_sync_mode: 0x12 },
    HdmiModeEntry { h_active: 640, v_active: 480, h_total: 800, v_total: 525, pclk_khz: 25_175, fps: 60, interlaced: false, out_width: 640, out_height: 480, enc_input_ctrl: 0x3E7C_0789, enc_sync_mode: 0x10 },
    HdmiModeEntry { h_active: 640, v_active: 480, h_total: 800, v_total: 480, pclk_khz: 24_000, fps: 60, interlaced: false, out_width: 640, out_height: 480, enc_input_ctrl: 0x3E7C_0689, enc_sync_mode: 0x12 },
];

/// Decoded live HDMI signal from MCU registers `0x1B`, `0x1C`, `0x1D`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HdmiSignal {
    pub width: u32,
    pub height: u32,
    pub h_total: u32,
    pub v_total: u32,
    pub refresh_hz: u32,
    pub interlaced: bool,
    pub hdcp: bool,
    pub audio: AudioSignalInfo,
    pub matched_mode_index: Option<usize>,
}

const PCLK_NUMERATOR_KHZ: f64 = 0x34BC00 as f64;
const STANDARD_REFRESH_RATES: [u32; 6] = [24, 25, 30, 50, 60, 120];

/// Parse the 7-byte MCU timing register block (`0x1D`) plus status registers
/// `0x1C` (HDMI lock & audio present), `0x24` (audio sample rate), and `0x25` (HDCP bit 0)
/// into an `HdmiSignal`.
pub fn parse_hdmi_timing(
    timing: &[u8; 7],
    reg_1c: u8,
    reg_24: u8,
    reg_25: u8,
) -> Option<HdmiSignal> {
    if (reg_1c & MCU_SIGNAL_HDMI_LOCK) == 0 {
        return None;
    }
    let h_total = (((timing[1] as u32) & 0x0F) << 8) | (timing[0] as u32);
    let h_active = (((timing[1] as u32) & 0xF0) << 4) | (timing[2] as u32);
    let v_total = (((timing[4] as u32) & 0x0F) << 8) | (timing[3] as u32);
    let v_active = (((timing[4] as u32) & 0xF0) << 4) | (timing[5] as u32);
    let pclk_div = timing[6] as u32;

    if pclk_div == 0 || h_total == 0 || v_total == 0 || h_active == 0 || v_active == 0 {
        return None;
    }

    let pclk_khz = PCLK_NUMERATOR_KHZ / (pclk_div as f64);
    let raw_hz = (pclk_khz * 1000.0) / ((h_total * v_total) as f64);
    let refresh_hz = *STANDARD_REFRESH_RATES
        .iter()
        .min_by(|a, b| (**a as f64 - raw_hz).abs().total_cmp(&(**b as f64 - raw_hz).abs()))?;

    let matched = match_hdmi_mode(h_active, v_active, h_total, v_total, refresh_hz);
    let (width, height, interlaced) = if let Some(idx) = matched {
        let m = HDMI_MODES[idx];
        (m.out_width as u32, m.out_height as u32, m.interlaced)
    } else {
        (h_active, v_active, false)
    };

    Some(HdmiSignal {
        width,
        height,
        h_total,
        v_total,
        refresh_hz,
        interlaced,
        hdcp: (reg_25 & MCU_HDCP_ACTIVE_BIT) != 0,
        audio: AudioSignalInfo::from_mcu_bytes(reg_24, reg_1c),
        matched_mode_index: matched,
    })
}

/// Match detected HDMI active/total dimensions and refresh rate against `HDMI_MODES`.
/// Follows `InterfaceNUC100::getHdmiVideo_6604`: exact `h_active`, `|v_active - entry.v_active| <= 10`,
/// then closest `h_total` / `v_total` / `fps`.
pub fn match_hdmi_mode(
    h_active: u32,
    v_active: u32,
    h_total: u32,
    v_total: u32,
    refresh_hz: u32,
) -> Option<usize> {
    HDMI_MODES
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.h_active as u32 == h_active
                && (m.v_active as i32 - v_active as i32).abs() <= 10
        })
        .min_by_key(|(_, m)| {
            let dfps = (m.fps as i32 - refresh_hz as i32).unsigned_abs() * 1000;
            let dh = (m.h_total as i32 - h_total as i32).unsigned_abs();
            let dv = (m.v_total as i32 - v_total as i32).unsigned_abs();
            dfps + dh + dv
        })
        .map(|(idx, _)| idx)
}

// ---------------------------------------------------------------------------
// Pure Encoder Register Calculations (H.264+AAC and Hardware MJPEG)
// ---------------------------------------------------------------------------

pub const ENC_FUNC_H264_AAC: u32 = 0x8000_0011;
pub const ENC_FUNC_MJPEG: u32 = 0x8000_0002;
pub const ENC_SYSTEM_LINK_DEFAULT: u32 = 0x0100_0100;

pub const BASE_SYS_CTRL_H264: u32 = 0x2101_C219;
pub const BASE_SYS_CTRL_MJPEG: u32 = 0x2101_B20E;
pub const BASE_RATE_CONTROL: u32 = 0x0078_7530;
pub const BASE_VBR_BITRATE: u32 = 0x1F40_07D0;
pub const BASE_FILTER_CONTROL: u32 = 0x0000_2000;
pub const BASE_GOP_LOOP_FILTER: u32 = 0xF199_001E;
pub const BASE_BLOCK_XFER_SIZE: u32 = 0x0000_0010;
pub const BASE_AUDIO_CONTROL_PARAM: u32 = 0x2116_1080;
pub const BASE_AUDIO_CONTROL_EX_AAC: u32 = 0x5208_40F4;
pub const AOU_PASS_EN_BIT: u32 = 1 << 16;

/// Complete hardware register block computed for starting either H.264+AAC or MJPEG capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderRegisters {
    pub enc_function: u32,
    pub sys_control: u32,
    pub in_resolution: u32,
    pub input_control: u32,
    pub rate_control: u32,
    pub vbr_bitrate: u32,
    pub filter_control: u32,
    pub gop_loop_filter: u32,
    pub out_resolution: u32,
    pub block_xfer_size: u32,
    pub audio_control: u32,
    pub audio_control_ex: u32,
    pub mjpeg_quality: Option<u32>,
    pub out_width: u32,
    pub out_height: u32,
    pub out_fps: u32,
}

/// Replace `out_frame_rate` (bits 23..28, verified in `AVer330USB.sys` `0x5e730`) of `EncInputControl`.
pub fn with_output_fps(base_input_ctrl: u32, out_fps: u32) -> u32 {
    let clamped = out_fps.clamp(15, 60) & 0x3F;
    (base_input_ctrl & !(0x3F << 23)) | (clamped << 23)
}

/// Build `EncRateControl` (`0x6EC`): upper 16 bits = macroblocks per row (`width / 16`,
/// verified in `AVer330USB.sys` `0x3a026..0x3a055` and `AVer835_x64.inf` `0x00501f40` for 720p,
/// `0x0078xxxx` for 1080p); lower 16 bits = target video bitrate in kbps.
pub fn encode_rate_control(out_width: u32, video_kbps: u32) -> u32 {
    let mb_per_row = (out_width / 16).clamp(16, 255);
    (mb_per_row << 16) | (video_kbps & 0xFFFF)
}

/// Build `EncGopLoopFilter` (`0x6E0`): sets keyframe interval (frames) in bits 0..15.
pub fn encode_gop_loop_filter(gop_frames: u32) -> u32 {
    (BASE_GOP_LOOP_FILTER & !0xFFFF) | (gop_frames & 0xFFFF)
}

/// Build `EncAudioControlParam` (`0x6D4`):
/// - bits 0..9: AAC bitrate (kbps)
/// - bits 10..11: channel select (0 = stereo)
/// - bits 12..15: sample rate selector (1 = 48 kHz, 2 = 44.1 kHz, 0 = 32 kHz)
/// - bit 16: `aou_pass_en` (3.5 mm AUDIO OUT jack passthrough)
pub fn encode_audio_control_param(audio_kbps: u32, sample_rate_code: u32, audio_out_jack: bool) -> u32 {
    let cleared = BASE_AUDIO_CONTROL_PARAM & !0x03FF & !(0x0F << 12) & !AOU_PASS_EN_BIT;
    let mut val = cleared | (audio_kbps & 0x03FF) | ((sample_rate_code & 0x0F) << 12);
    if audio_out_jack {
        val |= AOU_PASS_EN_BIT;
    }
    val
}

/// Compute the complete hardware register block for the given settings and detected HDMI signal.
pub fn compute_encoder_registers(settings: &Settings, signal: Option<&HdmiSignal>) -> EncoderRegisters {
    // Fallback mode when starting before/without lock: 1080p60 CEA (index 4).
    let mode_entry = signal
        .and_then(|s| s.matched_mode_index)
        .map(|idx| HDMI_MODES[idx])
        .unwrap_or_else(|| {
            if let Some(sig) = signal {
                if sig.width <= 1280 && sig.height <= 720 {
                    HDMI_MODES[7] // 720p60
                } else {
                    HDMI_MODES[4] // 1080p60
                }
            } else {
                HDMI_MODES[4]
            }
        });

    let in_w = signal.map(|s| s.width).unwrap_or(mode_entry.out_width as u32);
    let in_h = signal.map(|s| s.height).unwrap_or(mode_entry.out_height as u32);
    let in_hz = signal.map(|s| s.refresh_hz).unwrap_or(mode_entry.fps as u32);

    let in_res = ((in_h & 0xFFFF) << 16) | (in_w & 0xFFFF);

    let sr_code = match settings.audio_input {
        AudioInput::Jack => 1, // Analog ADC is clocked at 48 kHz
        AudioInput::Hdmi => signal
            .map(|s| s.audio.encoder_rate_code())
            .unwrap_or(1),
    };

    match settings.mode {
        CaptureMode::H264Aac => {
            let out_fps = settings.fps_policy.resolve(in_h, in_hz);
            let input_ctrl = if settings.fps_policy == FpsPolicy::Auto
                && signal.and_then(|s| s.matched_mode_index).is_some()
            {
                // Preserve exact verified driver register when Auto matches a known mode.
                mode_entry.enc_input_ctrl
            } else {
                with_output_fps(mode_entry.enc_input_ctrl, out_fps)
            };
            let gop_frames = settings.gop.frames(out_fps);
            let max_kbps = settings.video_kbps.clamp(8_000, 30_000);
            let min_kbps = (settings.video_kbps / 4).clamp(1_000, 4_000);

            EncoderRegisters {
                enc_function: ENC_FUNC_H264_AAC,
                sys_control: BASE_SYS_CTRL_H264,
                in_resolution: in_res,
                input_control: input_ctrl,
                rate_control: encode_rate_control(in_w, settings.video_kbps),
                vbr_bitrate: ((max_kbps & 0xFFFF) << 16) | (min_kbps & 0xFFFF),
                filter_control: BASE_FILTER_CONTROL,
                gop_loop_filter: encode_gop_loop_filter(gop_frames),
                out_resolution: in_res,
                block_xfer_size: BASE_BLOCK_XFER_SIZE,
                audio_control: encode_audio_control_param(
                    settings.audio_kbps,
                    sr_code,
                    settings.audio_out_jack,
                ),
                audio_control_ex: BASE_AUDIO_CONTROL_EX_AAC,
                mjpeg_quality: None,
                out_width: in_w,
                out_height: in_h,
                out_fps,
            }
        }
        CaptureMode::Mjpeg => {
            let (req_w, req_h) = settings.mjpeg_resolution.dimensions();
            // Do not upscale beyond input dimensions when input is smaller (e.g. 640x480 BIOS).
            let (out_w, out_h) = if in_w < req_w || in_h < req_h {
                (in_w, in_h)
            } else {
                (req_w, req_h)
            };
            let out_fps = settings.mjpeg_fps.min(in_hz.max(15)).clamp(15, 30);
            let out_res = ((out_h & 0xFFFF) << 16) | (out_w & 0xFFFF);

            EncoderRegisters {
                enc_function: ENC_FUNC_MJPEG,
                sys_control: BASE_SYS_CTRL_MJPEG,
                in_resolution: in_res,
                input_control: with_output_fps(mode_entry.enc_input_ctrl, out_fps),
                rate_control: encode_rate_control(out_w, settings.video_kbps),
                vbr_bitrate: BASE_VBR_BITRATE,
                filter_control: BASE_FILTER_CONTROL,
                gop_loop_filter: encode_gop_loop_filter(out_fps),
                out_resolution: out_res,
                block_xfer_size: BASE_BLOCK_XFER_SIZE,
                audio_control: encode_audio_control_param(
                    settings.audio_kbps,
                    sr_code,
                    settings.audio_out_jack,
                ),
                audio_control_ex: BASE_AUDIO_CONTROL_EX_AAC,
                mjpeg_quality: Some(settings.mjpeg_quality as u32),
                out_width: out_w,
                out_height: out_h,
                out_fps,
            }
        }
    }
}

/// MCU register 0x2E value for selecting HDMI IN (`0x04`) vs Jack 3.5 mm AUDIO IN (`0x05`).
pub fn mcu_audio_source_value(input: AudioInput) -> u8 {
    match input {
        AudioInput::Hdmi => 0x04,
        AudioInput::Jack => 0x05,
    }
}

/// MCU register 0x41 value for analog Jack 3.5 mm AUDIO IN gain (`0x80 | clamp(1..=11)`).
pub fn mcu_analog_volume_value(volume: u8) -> u8 {
    0x80 | volume.clamp(1, 11)
}

// ---------------------------------------------------------------------------
// Hardware Initialization Tables (Bridge Memory Windows + 206 + 5 I2C Writes)
// ---------------------------------------------------------------------------

/// QL202 memory window & SDRAM controller initialization sequence.
pub const BRIDGE_MEM_WINDOW_REGS: [(u32, u32); 11] = [
    (0x081C, 0x0000_4000),
    (0x0820, 0x0010_3FFF),
    (0x0824, 0x0000_0000),
    (0x0828, 0x0010_4000),
    (0x082C, 0x0020_3FFF),
    (0x0830, 0x0010_0000),
    (0x0834, 0x0020_4000),
    (0x0838, 0x0030_3FFF),
    (0x083C, 0x0020_0000),
    (0x0840, 0x7000_3124),
    (0x0840, 0x9000_3124),
];

/// Post-`SystemLink` 5 I2C writes to board audio/video routing chip `0x35`.
pub const POST_LINK_I2C_WRITES: [(u8, u8, u8); 5] = [
    (0x35, 0x14, 0x1F),
    (0x35, 0x15, 0xEC),
    (0x35, 0x1C, 0x49),
    (0x35, 0x1D, 0x00),
    (0x35, 0x5A, 0x01),
];

/// Build the official 206 I2C board initialization writes `(chip_addr, reg, val)`
/// for chips `0x31` (3 writes), `0x35` (5 writes), `0x21` (22 writes), and `0x3D` (176 writes).
pub fn build_init_i2c_writes() -> Vec<(u8, u8, u8)> {
    let mut w = Vec::with_capacity(206);

    // Chip 0x31 (3 writes)
    for &(r, v) in &[(0xF0, 0x10), (0xF1, 0x0F), (0xF4, 0x20)] {
        w.push((0x31, r, v));
    }

    // Chip 0x35 (5 writes)
    for &(r, v) in &[
        (0x14, 0x1F),
        (0x15, 0xEC),
        (0x1C, 0x49),
        (0x1D, 0x04),
        (0x5A, 0x01),
    ] {
        w.push((0x35, r, v));
    }

    // Chip 0x21 (22 writes)
    for &(r, v) in &[
        (0x00, 0x00),
        (0x03, 0x09),
        (0x04, 0x47),
        (0x05, 0x06),
        (0x06, 0x02),
        (0x17, 0x01),
        (0x1D, 0x40),
        (0x31, 0x12),
        (0x34, 0x00),
        (0x35, 0x02),
        (0x37, 0x00),
        (0x3A, 0x01),
        (0x3C, 0x58),
        (0x47, 0x00),
        (0x68, 0xF0),
        (0x69, 0x00),
        (0x6B, 0xE3),
        (0x7B, 0x0D),
        (0xBA, 0xA0),
        (0xC8, 0x08),
        (0xF3, 0x00),
        (0xF4, 0x3F),
    ] {
        w.push((0x21, r, v));
    }

    // Chip 0x3D (176 writes)
    for &(r, v) in &[
        (0x01, 0x00),
        (0x02, 0x18),
        (0x03, 0x00),
        (0x07, 0x00),
        (0x08, 0x00),
        (0x09, 0x00),
        (0x0A, 0x0D),
        (0x0B, 0x0E),
        (0x0C, 0x04),
        (0x0D, 0x18),
        (0x0E, 0x01),
        (0x0F, 0x13),
        (0x10, 0x25),
        (0x11, 0x37),
        (0x12, 0x00),
        (0x13, 0x00),
        (0x14, 0x00),
        (0x15, 0x22),
        (0x16, 0x40),
        (0x17, 0x0A),
        (0x18, 0x06),
        (0x19, 0x62),
        (0x1A, 0x04),
        (0x1B, 0xA8),
        (0x1C, 0x00),
        (0x1D, 0x00),
        (0x1E, 0x1C),
        (0x1F, 0x84),
        (0x20, 0x1C),
        (0x21, 0xBF),
        (0x22, 0x04),
        (0x23, 0xA8),
        (0x24, 0x1E),
        (0x25, 0x70),
        (0x26, 0x02),
        (0x27, 0x1E),
        (0x28, 0x00),
        (0x29, 0x00),
        (0x2A, 0x04),
        (0x2B, 0xA8),
        (0x2C, 0x08),
        (0x2D, 0x12),
        (0x2E, 0x1B),
        (0x2F, 0xAC),
    ] {
        w.push((0x3D, r, v));
    }
    for r in 0x30u8..=0x3A {
        w.push((0x3D, r, 0x00));
    }
    for &(r, v) in &[
        (0x3B, 0x80),
        (0x3C, 0x00),
        (0x40, 0x00),
        (0x41, 0x10),
        (0x43, 0xA0),
        (0x44, 0xF8),
        (0x45, 0x20),
    ] {
        w.push((0x3D, r, v));
    }
    for r in 0x46u8..=0x93 {
        w.push((0x3D, r, 0x00));
    }
    for &(r, v) in &[
        (0x94, 0xC0),
        (0x95, 0x00),
        (0x96, 0x20),
        (0x97, 0x00),
        (0x98, 0x03),
        (0x99, 0x02),
        (0x9A, 0x00),
        (0x9B, 0x18),
        (0x9C, 0x38),
        (0x9D, 0x61),
        (0xA1, 0x00),
        (0xA2, 0x87),
        (0xA3, 0x87),
        (0xA4, 0x08),
        (0xA5, 0xC0),
    ] {
        w.push((0x3D, r, v));
    }
    for r in 0xA6u8..=0xAB {
        w.push((0x3D, r, 0x00));
    }
    for &(r, v) in &[
        (0xAF, 0x16),
        (0xB9, 0x00),
        (0xBA, 0x60),
        (0xBB, 0x00),
        (0xC4, 0x00),
        (0xC9, 0x03),
        (0xCA, 0x00),
        (0xCB, 0x00),
        (0xCC, 0x02),
        (0xFA, 0x7D),
        (0xFB, 0xAA),
        (0xFC, 0x1C),
        (0xFD, 0x00),
        (0xFE, 0xB0),
        (0xFF, 0x00),
    ] {
        w.push((0x3D, r, v));
    }

    w
}

/// Convert a decimal 0..=99 value to BCD for MCU RTC register `0x30`.
pub fn to_bcd(v: u32) -> u8 {
    (((v / 10) % 10) << 4 | (v % 10)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{GopSeconds, MjpegResolution};

    #[test]
    fn i2c_init_table_has_exact_206_writes() {
        let writes = build_init_i2c_writes();
        assert_eq!(writes.len(), 206);
        assert_eq!(writes.iter().filter(|w| w.0 == 0x31).count(), 3);
        assert_eq!(writes.iter().filter(|w| w.0 == 0x35).count(), 5);
        assert_eq!(writes.iter().filter(|w| w.0 == 0x21).count(), 22);
        assert_eq!(writes.iter().filter(|w| w.0 == 0x3D).count(), 176);
        assert_eq!(POST_LINK_I2C_WRITES.len(), 5);
    }

    #[test]
    fn safety_guard_blocks_0x47_and_0x49() {
        assert!(check_mcu_write_safety(0x47).is_err());
        assert!(check_mcu_write_safety(0x49).is_err());
        assert!(check_mcu_write_safety(0x2E).is_ok());
        assert!(check_mcu_write_safety(0x2C).is_ok());
        assert!(check_mcu_write_safety(0x41).is_ok());

        let bad_47 = [0x0B, 0x01, 0x02, 0x00, 0x15, 0, 0, 0, 0x47, 0x01];
        let bad_49 = [0x0B, 0x01, 0x02, 0x00, 0x15, 0, 0, 0, 0x49, 0x80];
        let ok_2e = [0x0B, 0x01, 0x02, 0x00, 0x15, 0, 0, 0, 0x2E, 0x05];
        assert!(check_command_safety(&bad_47).is_err());
        assert!(check_command_safety(&bad_49).is_err());
        assert!(check_command_safety(&ok_2e).is_ok());
    }

    #[test]
    fn hdmi_mode_table_matches_cea_and_vesa_bios_modes() {
        assert_eq!(HDMI_MODES.len(), 49);
        // 1080p60 CEA
        let idx_1080 = match_hdmi_mode(1920, 1080, 2200, 1125, 60).unwrap();
        assert_eq!(idx_1080, 4);
        assert_eq!(HDMI_MODES[idx_1080].enc_input_ctrl, 0x0F7C_0609);

        // 720p60 CEA
        let idx_720 = match_hdmi_mode(1280, 720, 1650, 750, 60).unwrap();
        assert_eq!(idx_720, 7);
        assert_eq!(HDMI_MODES[idx_720].enc_input_ctrl, 0x3E7C_0609);

        // BIOS/UEFI VESA modes: 1024x768, 800x600, 640x480
        let idx_xga = match_hdmi_mode(1024, 768, 1344, 806, 60).unwrap();
        assert_eq!(HDMI_MODES[idx_xga].out_width, 1024);
        assert_eq!(HDMI_MODES[idx_xga].out_height, 768);

        let idx_svga = match_hdmi_mode(800, 600, 1056, 628, 60).unwrap();
        assert_eq!(HDMI_MODES[idx_svga].out_width, 800);
        assert_eq!(HDMI_MODES[idx_svga].out_height, 600);

        let idx_vga = match_hdmi_mode(640, 480, 800, 525, 60).unwrap();
        assert_eq!(HDMI_MODES[idx_vga].out_width, 640);
        assert_eq!(HDMI_MODES[idx_vga].out_height, 480);
    }

    #[test]
    fn parse_1080p60_timing_and_no_signal() {
        let raw_1080p60 = [0x98, 0x78, 0x80, 0x65, 0x44, 0x38, 0x17];
        let sig = parse_hdmi_timing(
            &raw_1080p60,
            MCU_SIGNAL_HDMI_LOCK | MCU_SIGNAL_HDMI_AUDIO,
            0x02, // 48 kHz on MCU 0x24
            0x00, // HDCP off on MCU 0x25
        )
        .unwrap();
        assert_eq!(sig.width, 1920);
        assert_eq!(sig.height, 1080);
        assert_eq!(sig.refresh_hz, 60);
        assert!(!sig.hdcp);
        assert!(sig.audio.present);
        assert_eq!(sig.audio.sample_rate_hz, 48_000);
        assert_eq!(sig.audio.encoder_rate_code(), 1);
        assert!(!sig.audio.compressed_non_pcm);
        assert_eq!(sig.matched_mode_index, Some(4));

        // HDCP is bit 0 of MCU 0x25 (verified in getHdmiVideo_HDCP 0x31126)
        let sig_hdcp = parse_hdmi_timing(
            &raw_1080p60,
            MCU_SIGNAL_HDMI_LOCK | MCU_SIGNAL_HDMI_AUDIO,
            0x00, // 44.1 kHz -> code 0
            MCU_HDCP_ACTIVE_BIT,
        )
        .unwrap();
        assert!(sig_hdcp.hdcp);
        assert_eq!(sig_hdcp.audio.sample_rate_hz, 44_100);
        assert_eq!(sig_hdcp.audio.encoder_rate_code(), 0);

        // 32 kHz -> code 2 (verified in AVer330USB.sys 0x39fbb)
        let sig_32k = parse_hdmi_timing(
            &raw_1080p60,
            MCU_SIGNAL_HDMI_LOCK | MCU_SIGNAL_HDMI_AUDIO,
            0x03,
            0x00,
        )
        .unwrap();
        assert_eq!(sig_32k.audio.sample_rate_hz, 32_000);
        assert_eq!(sig_32k.audio.encoder_rate_code(), 2);

        // No HDMI lock bit -> None
        assert!(parse_hdmi_timing(&raw_1080p60, 0x00, 0x02, 0x00).is_none());
        // Zero clock divider -> None
        let zero_clk = [0x98, 0x78, 0x80, 0x65, 0x44, 0x38, 0x00];
        assert!(parse_hdmi_timing(&zero_clk, MCU_SIGNAL_HDMI_LOCK, 0x02, 0x00).is_none());
    }

    #[test]
    fn encoder_registers_h264_and_mjpeg_bitfields() {
        let mut s = Settings {
            video_kbps: 8_000,
            gop: GopSeconds::TwoSec,
            audio_kbps: 192,
            audio_out_jack: true,
            ..Default::default()
        };

        let raw_720p60 = [0x72, 0x56, 0x00, 0xEE, 0x22, 0xD0, 0x2E];
        let sig_720 = parse_hdmi_timing(
            &raw_720p60,
            MCU_SIGNAL_HDMI_LOCK | MCU_SIGNAL_HDMI_AUDIO,
            0x02,
            0x00,
        )
        .unwrap();
        assert_eq!(sig_720.width, 1280);
        assert_eq!(sig_720.height, 720);

        // Auto mode on 720p60 keeps 60 fps and uses 1280/16 = 0x50 macroblocks/row (matching AVer835_x64.inf 0x00501F40)
        let r_auto = compute_encoder_registers(&s, Some(&sig_720));
        assert_eq!(r_auto.enc_function, ENC_FUNC_H264_AAC);
        assert_eq!(r_auto.out_fps, 60);
        assert_eq!(r_auto.input_control, 0x3E7C_0609);
        assert_eq!(r_auto.rate_control, 0x0050_1F40);
        assert_eq!(r_auto.audio_control, 0x2117_10C0);

        // Force30 on 720p60 sets bits 23..28 to 30
        s.fps_policy = FpsPolicy::Force30;
        let r_f30 = compute_encoder_registers(&s, Some(&sig_720));
        assert_eq!(r_f30.out_fps, 30);
        assert_eq!((r_f30.input_control >> 23) & 0x3F, 30);

        // 1080p60 uses 1920/16 = 0x78 macroblocks/row
        let raw_1080p60 = [0x98, 0x78, 0x80, 0x65, 0x44, 0x38, 0x17];
        let sig_1080 = parse_hdmi_timing(&raw_1080p60, MCU_SIGNAL_HDMI_LOCK, 0x02, 0x00).unwrap();
        let r_1080 = compute_encoder_registers(&s, Some(&sig_1080));
        assert_eq!(r_1080.rate_control, 0x0078_1F40);

        // MJPEG mode uses 0x8000_0002, 0x2101_B20E, scales 1080p -> 720p30
        s.mode = CaptureMode::Mjpeg;
        s.mjpeg_resolution = MjpegResolution::R1280x720;
        s.mjpeg_fps = 30;
        s.mjpeg_quality = 12;
        let r_mjpeg = compute_encoder_registers(&s, Some(&sig_1080));
        assert_eq!(r_mjpeg.enc_function, ENC_FUNC_MJPEG);
        assert_eq!(r_mjpeg.sys_control, BASE_SYS_CTRL_MJPEG);
        assert_eq!(r_mjpeg.out_resolution, (720 << 16) | 1280);
        assert_eq!(r_mjpeg.rate_control, 0x0050_1F40);
        assert_eq!(r_mjpeg.mjpeg_quality, Some(12));
        assert_eq!((r_mjpeg.input_control >> 23) & 0x3F, 30);
    }

    #[test]
    fn audio_source_and_analog_volume_encoding() {
        assert_eq!(mcu_audio_source_value(AudioInput::Hdmi), 0x04);
        assert_eq!(mcu_audio_source_value(AudioInput::Jack), 0x05);
        assert_eq!(mcu_analog_volume_value(1), 0x81);
        assert_eq!(mcu_analog_volume_value(11), 0x8B);
        assert_eq!(mcu_analog_volume_value(0), 0x81);
        assert_eq!(mcu_analog_volume_value(99), 0x8B);
    }
}
