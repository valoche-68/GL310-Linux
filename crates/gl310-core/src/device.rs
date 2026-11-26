//! Userspace USB (`rusb` / `libusb-1.0`) hardware driver for AVerMedia LGP Lite (GL310, `07ca:c835`).
//!
//! Safety guarantee:
//! - Firmware is loaded strictly into volatile ARM RAM (unplugging for 10 s resets the board).
//! - Every low-level command path enforces `check_command_safety` / `check_mcu_write_safety`,
//!   unconditionally rejecting permanent MCU profile registers `0x47` and `0x49`.

use std::fmt;
use std::thread::sleep;
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike};
use rusb::{Context, DeviceHandle, UsbContext};
use serde::{Deserialize, Serialize};

use crate::firmware::VerifiedFirmware;
use crate::proto::{
    self, check_command_safety, check_mcu_write_safety, compute_encoder_registers,
    mcu_analog_volume_value, mcu_audio_source_value, parse_hdmi_timing, to_bcd, EncoderRegisters,
    HdmiSignal, McuEvent, ARM_AUDIO_BSS_CLEAR_CHUNKS, ARM_AUDIO_FW_WORD_BASE, ARM_CFG_BASE_WORD,
    ARM_CFG_FIXED_MODE_WORD, ARM_CFG_INT_MODE_WORD, ARM_QSOS_MAGIC, ARM_QSOS_MAGIC_WORD,
    ARM_VIDEO_FW_WORD_BASE, BRIDGE_MEM_WINDOW_REGS, DMA_BLOCK_WORDS, ENC_SYSTEM_LINK_DEFAULT,
    EP_CMD_IN, EP_CMD_OUT, EP_DATA_IN, EP_DATA_OUT, EXPECTED_CHIP_REV, GPIO_MCU_INT_BIT,
    IRQ_ARM_MAILBOX_BIT, MBOX_ACK_HIGH, MBOX_ACK_LOW, MBOX_COMPLETE_BUFFER, MBOX_PING,
    MBOX_SET_AUDIO_INPUT_VOL, MBOX_SET_EXT_CONFIG, MBOX_START_ENCODER, MBOX_STOP_ENCODER,
    MBOX_SYSTEM_CLOSE, MBOX_SYSTEM_LINK, MBOX_SYSTEM_OPEN, MCU_I2C_ADDR, MCU_REG_ANALOG_VOL,
    MCU_REG_AUDIO_RATE, MCU_REG_AUDIO_SOURCE, MCU_REG_BUSY_STATUS, MCU_REG_EVENT,
    MCU_REG_HDCP_STATUS, MCU_REG_IDENTITY, MCU_REG_LED, MCU_REG_RTC, MCU_REG_SIGNAL_STATUS,
    MCU_REG_TIMING_BLOCK, MCU_REG_WAKE, MSG_KIND_FRAME, POST_LINK_I2C_WRITES, REG_CHIP_REV,
    REG_FROM_ARM_MSG, REG_FROM_ARM_STATUS, REG_GPIO_DATA, REG_GPIO_DIR, REG_GPIO_IN, REG_GPIO_OUT,
    REG_IRQ_STATUS, REG_SYS_CTRL_50, REG_TO_ARM_AUDIO_CTRL, REG_TO_ARM_AUDIO_EX, REG_TO_ARM_MSG,
    REG_TO_ARM_PARAM0, REG_TO_ARM_PARAM1, REG_TO_ARM_PARAM2, REG_TO_ARM_PARAM3, REG_TO_ARM_PARAM4,
    REG_TO_ARM_PARAM5, REG_TO_ARM_PARAM6, REG_TO_ARM_PARAM7, REG_TO_ARM_PARAM8, REG_TO_ARM_STATUS,
    STREAM_TAG_TS, STREAM_TAG_VIDEO, USB_PRODUCT_ID, USB_PRODUCT_ID_LOCKED, USB_VENDOR_ID,
};
use crate::settings::{AudioInput, CaptureMode, LedMode, Settings};
use crate::stream::bswap32_in_place;

const CMD_TIMEOUT: Duration = Duration::from_millis(1_000);
const DATA_TIMEOUT: Duration = Duration::from_millis(5_000);

#[derive(Debug)]
pub enum DeviceError {
    Usb(rusb::Error),
    NotFound,
    LockedD835,
    SafetyBlocked(String),
    Protocol(String),
}

impl From<rusb::Error> for DeviceError {
    fn from(e: rusb::Error) -> Self {
        DeviceError::Usb(e)
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::Usb(rusb::Error::Access) => write!(
                f,
                "USB permission denied for 07ca:c835 (install udev rule: sudo bash scripts/install_udev.sh)"
            ),
            DeviceError::Usb(rusb::Error::Busy) => write!(
                f,
                "GL310 is already claimed by another process or kernel driver"
            ),
            DeviceError::Usb(e) => write!(f, "USB error: {e}"),
            DeviceError::NotFound => write!(f, "No AVerMedia GL310 (07ca:c835) found on USB"),
            DeviceError::LockedD835 => write!(
                f,
                "Device is in locked 07ca:d835 state — unplug USB for 10 seconds and reconnect"
            ),
            DeviceError::SafetyBlocked(msg) => write!(f, "{msg}"),
            DeviceError::Protocol(msg) => write!(f, "Protocol error: {msg}"),
        }
    }
}

impl std::error::Error for DeviceError {}

pub type Result<T> = std::result::Result<T, DeviceError>;

/// USB presence check without claiming the interface (usable even without udev permissions).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsbProbeInfo {
    pub present: bool,
    pub locked_d835: bool,
    pub bus: Option<u8>,
    pub address: Option<u8>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub can_open: bool,
    pub open_error: Option<String>,
}

/// Hardware identity summary retrieved from an opened GL310.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub bus: u8,
    pub address: u8,
    pub chip_revision: u32,
    pub chip_revision_ok: bool,
    pub mcu_identity: String,
    pub firmware_loaded: bool,
    pub audio_source: AudioInput,
    pub signal: Option<HdmiSignal>,
}

/// Raw chunk returned by a single mailbox poll cycle.
#[derive(Clone, Debug)]
pub struct StreamChunk {
    pub tag: u32,
    pub pts: u32,
    pub data: Vec<u8>,
}

/// Summary of the no-HDMI dry-test hardware verification sequence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryTestReport {
    pub usb_ok: bool,
    pub chip_revision: u32,
    pub chip_revision_ok: bool,
    pub mcu_identity: String,
    pub leds_cycled: bool,
    pub button_tested: bool,
    pub button_clicked: bool,
    pub firmware_booted: bool,
    pub qsos_verified: bool,
    pub mailbox_ping_ok: bool,
    pub safety_guard_ok: bool,
    pub notes: Vec<String>,
}

/// Probe USB bus for `07ca:c835` / `07ca:d835` and test whether non-root open succeeds.
pub fn probe_usb() -> UsbProbeInfo {
    let ctx = match Context::new() {
        Ok(c) => c,
        Err(e) => {
            return UsbProbeInfo {
                present: false,
                locked_d835: false,
                bus: None,
                address: None,
                vendor_id: None,
                product_id: None,
                can_open: false,
                open_error: Some(format!("libusb init failed: {e}")),
            };
        }
    };
    let devices = match ctx.devices() {
        Ok(d) => d,
        Err(e) => {
            return UsbProbeInfo {
                present: false,
                locked_d835: false,
                bus: None,
                address: None,
                vendor_id: None,
                product_id: None,
                can_open: false,
                open_error: Some(format!("USB enumeration failed: {e}")),
            };
        }
    };

    for dev in devices.iter() {
        let Ok(desc) = dev.device_descriptor() else {
            continue;
        };
        if desc.vendor_id() != USB_VENDOR_ID {
            continue;
        }
        if desc.product_id() == USB_PRODUCT_ID_LOCKED {
            return UsbProbeInfo {
                present: true,
                locked_d835: true,
                bus: Some(dev.bus_number()),
                address: Some(dev.address()),
                vendor_id: Some(desc.vendor_id()),
                product_id: Some(desc.product_id()),
                can_open: false,
                open_error: Some(DeviceError::LockedD835.to_string()),
            };
        }
        if desc.product_id() == USB_PRODUCT_ID {
            let (can_open, open_error) = match dev.open() {
                Ok(_) => (true, None),
                Err(e) => (false, Some(DeviceError::Usb(e).to_string())),
            };
            return UsbProbeInfo {
                present: true,
                locked_d835: false,
                bus: Some(dev.bus_number()),
                address: Some(dev.address()),
                vendor_id: Some(desc.vendor_id()),
                product_id: Some(desc.product_id()),
                can_open,
                open_error,
            };
        }
    }

    UsbProbeInfo {
        present: false,
        locked_d835: false,
        bus: None,
        address: None,
        vendor_id: None,
        product_id: None,
        can_open: false,
        open_error: Some(DeviceError::NotFound.to_string()),
    }
}

/// Active handle to an opened AVerMedia GL310 (`07ca:c835`).
pub struct Gl310Device {
    handle: DeviceHandle<Context>,
    bus: u8,
    address: u8,
}

impl Gl310Device {
    /// Open and claim interface 0 of the first attached GL310 (`07ca:c835`).
    pub fn open() -> Result<Self> {
        let ctx = Context::new()?;
        for dev in ctx.devices()?.iter() {
            let desc = dev.device_descriptor()?;
            if desc.vendor_id() != USB_VENDOR_ID {
                continue;
            }
            if desc.product_id() == USB_PRODUCT_ID_LOCKED {
                return Err(DeviceError::LockedD835);
            }
            if desc.product_id() == USB_PRODUCT_ID {
                let bus = dev.bus_number();
                let address = dev.address();
                let handle = dev.open()?;
                // Detach kernel driver if any was bound (e.g. out-of-tree kernel module).
                if handle.kernel_driver_active(0).unwrap_or(false) {
                    let _ = handle.detach_kernel_driver(0);
                }
                handle.claim_interface(0)?;
                let gl310 = Self {
                    handle,
                    bus,
                    address,
                };
                gl310.flush_endpoints();
                return Ok(gl310);
            }
        }
        Err(DeviceError::NotFound)
    }

    pub fn bus(&self) -> u8 {
        self.bus
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    /// Drain stale buffered packets left over from an interrupted session.
    pub fn flush_endpoints(&self) {
        let mut buf = vec![0u8; 65_536];
        for ep in [EP_DATA_IN, EP_CMD_IN] {
            for _ in 0..8 {
                if self
                    .handle
                    .read_bulk(ep, &mut buf, Duration::from_millis(30))
                    .is_err()
                {
                    break;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Low-Level USB Transport (with unconditional MCU 0x47/0x49 safety guard)
    // -----------------------------------------------------------------------

    fn send_cmd(&self, cmd: &[u8], expect_reply: bool) -> Result<Vec<u8>> {
        check_command_safety(cmd).map_err(DeviceError::SafetyBlocked)?;
        self.handle.write_bulk(EP_CMD_OUT, cmd, CMD_TIMEOUT)?;
        if !expect_reply {
            return Ok(Vec::new());
        }
        let mut reply = vec![0u8; 512];
        let n = self.handle.read_bulk(EP_CMD_IN, &mut reply, CMD_TIMEOUT)?;
        reply.truncate(n);
        Ok(reply)
    }

    pub fn reg_write(&self, reg: u32, val: u32) -> Result<()> {
        let mut cmd = Vec::with_capacity(12);
        cmd.extend_from_slice(&[0x01, 0x01, 0x01, 0x00]);
        cmd.extend_from_slice(&reg.to_le_bytes());
        cmd.extend_from_slice(&val.to_le_bytes());
        self.send_cmd(&cmd, false)?;
        Ok(())
    }

    pub fn reg_read_multi(&self, reg: u32, count: u8) -> Result<Vec<u32>> {
        let mut cmd = Vec::with_capacity(8);
        cmd.extend_from_slice(&[0x01, 0x00, count, 0x00]);
        cmd.extend_from_slice(&reg.to_le_bytes());
        let reply = self.send_cmd(&cmd, true)?;
        let need = 4 * count as usize;
        if reply.len() < need {
            return Err(DeviceError::Protocol(format!(
                "Short register read at {reg:#06x}: expected {need} bytes, got {}",
                reply.len()
            )));
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            out.push(u32::from_le_bytes([
                reply[4 * i],
                reply[4 * i + 1],
                reply[4 * i + 2],
                reply[4 * i + 3],
            ]));
        }
        Ok(out)
    }

    pub fn reg_read(&self, reg: u32) -> Result<u32> {
        Ok(self.reg_read_multi(reg, 1)?[0])
    }

    pub fn arm_set_run(&self, run: bool) -> Result<()> {
        self.send_cmd(&[0x07, u8::from(run), 0, 0, 0, 0, 0, 0], false)?;
        Ok(())
    }

    pub fn word_write(&self, word_addr: u32, val: u32) -> Result<()> {
        let mut cmd = Vec::with_capacity(16);
        cmd.extend_from_slice(&[0x02, 0x01, 0x04, 0x00]);
        cmd.extend_from_slice(&word_addr.to_le_bytes());
        cmd.extend_from_slice(&word_addr.to_le_bytes());
        cmd.extend_from_slice(&val.to_le_bytes());
        self.send_cmd(&cmd, false)?;
        Ok(())
    }

    pub fn word_read_bytes(&self, word_addr: u32) -> Result<Vec<u8>> {
        let mut cmd = Vec::with_capacity(12);
        cmd.extend_from_slice(&[0x02, 0x00, 0x04, 0x00]);
        cmd.extend_from_slice(&word_addr.to_le_bytes());
        cmd.extend_from_slice(&word_addr.to_le_bytes());
        self.send_cmd(&cmd, true)
    }

    fn dma_setup(&self, write: bool, word_addr: u32, words: u32) -> Result<()> {
        let mut cmd = Vec::with_capacity(16);
        cmd.extend_from_slice(&[0x09, u8::from(write), 0x08, 0x00, 0, 0, 0, 0]);
        cmd.extend_from_slice(&word_addr.to_le_bytes());
        cmd.extend_from_slice(&words.to_le_bytes());
        self.send_cmd(&cmd, true)?;
        Ok(())
    }

    fn read_data_exact(&self, bytes: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; bytes];
        let mut off = 0;
        while off < bytes {
            let n = self.handle.read_bulk(EP_DATA_IN, &mut buf[off..], DATA_TIMEOUT)?;
            if n == 0 {
                return Err(DeviceError::Protocol("Empty DMA read on EP 0x81".into()));
            }
            off += n;
        }
        Ok(buf)
    }

    pub fn dma_read(&self, word_addr: u32, bytes: usize) -> Result<Vec<u8>> {
        let words = bytes.div_ceil(4) as u32;
        let mut out = Vec::with_capacity(words as usize * 4);
        let mut done = 0u32;
        while done < words {
            let n = DMA_BLOCK_WORDS.min(words - done);
            self.dma_setup(false, word_addr + done, n)?;
            out.extend_from_slice(&self.read_data_exact(n as usize * 4)?);
            done += n;
        }
        out.truncate(bytes);
        Ok(out)
    }

    pub fn dma_write(&self, word_addr: u32, data: &[u8], verify_readback: bool) -> Result<()> {
        if !data.len().is_multiple_of(4) {
            return Err(DeviceError::Protocol("DMA write length must be a multiple of 4".into()));
        }
        for (idx, chunk) in data.chunks(DMA_BLOCK_WORDS as usize * 4).enumerate() {
            let addr = word_addr + idx as u32 * DMA_BLOCK_WORDS;
            let words = (chunk.len() / 4) as u32;
            self.dma_setup(true, addr, words)?;
            self.handle.write_bulk(EP_DATA_OUT, chunk, DATA_TIMEOUT)?;
            if verify_readback {
                self.dma_setup(false, addr, words)?;
                let back = self.read_data_exact(chunk.len())?;
                if back != chunk {
                    return Err(DeviceError::Protocol(format!(
                        "DMA readback verification mismatch at ARM word {addr:#x}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Write a 2-byte `(reg, val)` pair to an on-board I2C device (opcode `0x05`).
    pub fn i2c_write(&self, chip_addr: u8, reg: u8, val: u8) -> Result<()> {
        let cmd = [0x05, 0x01, 0x02, 0x00, chip_addr, 0, 0, 0, reg, val];
        self.send_cmd(&cmd, true)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // NUC100 MCU Access (I2C 0x15 via USB opcodes 0x0B / 0x0C)
    // -----------------------------------------------------------------------

    pub fn mcu_read(&self, reg: u8, len: u8) -> Result<Vec<u8>> {
        let cmd = [0x0C, 0x01, len, 0x00, MCU_I2C_ADDR, 0, 0, 0, reg];
        let ans = self.send_cmd(&cmd, true)?;
        if ans.len() < len as usize {
            return Err(DeviceError::Protocol(format!(
                "Short MCU read on reg {reg:#04x}: expected {len} bytes, got {}",
                ans.len()
            )));
        }
        Ok(ans[..len as usize].to_vec())
    }

    /// Write `payload` bytes starting at MCU register `reg`.
    /// Unconditionally blocked for permanent profile registers `0x47` and `0x49`.
    pub fn mcu_write(&self, reg: u8, payload: &[u8]) -> Result<()> {
        check_mcu_write_safety(reg).map_err(DeviceError::SafetyBlocked)?;
        let total = 1usize + payload.len();
        if total > 32 {
            return Err(DeviceError::Protocol("MCU write payload too large".into()));
        }
        let mut cmd = Vec::with_capacity(8 + total);
        cmd.extend_from_slice(&[0x0B, 0x01, total as u8, 0x00, MCU_I2C_ADDR, 0, 0, 0, reg]);
        cmd.extend_from_slice(payload);
        self.send_cmd(&cmd, true)?;
        Ok(())
    }

    pub fn mcu_write_u8(&self, reg: u8, val: u8) -> Result<()> {
        self.mcu_write(reg, &[val])
    }

    /// Read chip revision register `0x0038` (expected `0x00010020`).
    pub fn read_chip_revision(&self) -> Result<u32> {
        self.reg_read(REG_CHIP_REV)
    }

    /// Read ASCII MCU model identity string (e.g. `"G310"` at MCU `0x0A..0x0D`).
    pub fn read_mcu_identity(&self) -> Result<String> {
        let raw = self.mcu_read(MCU_REG_IDENTITY, 6)?;
        let text: String = raw
            .into_iter()
            .take_while(|&b| b != 0)
            .filter(|b| b.is_ascii_graphic() || *b == b' ')
            .map(|b| b as char)
            .collect();
        Ok(text)
    }

    pub fn firmware_loaded(&self) -> Result<bool> {
        let bytes = self.word_read_bytes(ARM_QSOS_MAGIC_WORD)?;
        Ok(bytes.starts_with(ARM_QSOS_MAGIC))
    }

    /// Set the physical LED ring animation (`0x03`, `0x04`, `0x05`, `0x0B`).
    pub fn set_led_code(&self, code: u8) -> Result<()> {
        self.mcu_write_u8(MCU_REG_LED, code)
    }

    pub fn set_led_mode(&self, mode: LedMode) -> Result<()> {
        if let Some(code) = mode.mcu_code() {
            self.set_led_code(code)?;
        }
        Ok(())
    }

    /// Select hardware audio input source (`HDMI IN = 0x04` or `Jack 3.5 mm AUDIO IN = 0x05`).
    pub fn set_audio_input(&self, input: AudioInput) -> Result<()> {
        self.mcu_write_u8(MCU_REG_AUDIO_SOURCE, mcu_audio_source_value(input))
    }

    pub fn get_audio_input(&self) -> Result<AudioInput> {
        let b = self.mcu_read(MCU_REG_AUDIO_SOURCE, 1)?[0];
        if (b & 0x01) != 0 {
            Ok(AudioInput::Jack)
        } else {
            Ok(AudioInput::Hdmi)
        }
    }

    /// Set analog 3.5 mm Jack AUDIO IN gain (`1..=11`, written as `0x80 | vol` to MCU `0x41`).
    pub fn set_analog_input_volume(&self, volume: u8) -> Result<()> {
        self.mcu_write_u8(MCU_REG_ANALOG_VOL, mcu_analog_volume_value(volume))
    }

    /// Update the MCU's RTC clock (`0x30`, 6 BCD bytes: YY MM DD hh mm ss).
    pub fn sync_mcu_rtc(&self) -> Result<()> {
        let now = chrono::Local::now();
        let bcd = [
            to_bcd((now.year() as u32) % 100),
            to_bcd(now.month()),
            to_bcd(now.day()),
            to_bcd(now.hour()),
            to_bcd(now.minute()),
            to_bcd(now.second()),
        ];
        self.mcu_write(MCU_REG_RTC, &bcd)
    }

    /// Wait until MCU busy/transition register `0x1B` reads `0x00` (up to 10 retries × 30 ms,
    /// matching `InterfaceNUC100::getHdmiVideo_6604` / `getHdmiAudio_6604` in `AVer330USB.sys`).
    fn wait_mcu_stable(&self) -> Result<()> {
        for _ in 0..10 {
            if self.mcu_read(MCU_REG_BUSY_STATUS, 1)?[0] == 0 {
                return Ok(());
            }
            sleep(Duration::from_millis(30));
        }
        Ok(())
    }

    /// Query current HDMI video & audio lock status, sample rate (`0x24`), HDCP (`0x25`),
    /// and 7-byte timing block (`0x1D`).
    pub fn read_signal(&self) -> Result<Option<HdmiSignal>> {
        self.wait_mcu_stable()?;
        let reg_1c = self.mcu_read(MCU_REG_SIGNAL_STATUS, 1)?[0];
        if (reg_1c & proto::MCU_SIGNAL_HDMI_LOCK) == 0 {
            return Ok(None);
        }
        let reg_24 = self.mcu_read(MCU_REG_AUDIO_RATE, 1)?[0];
        let reg_25 = self.mcu_read(MCU_REG_HDCP_STATUS, 1)?[0];
        let raw_timing = self.mcu_read(MCU_REG_TIMING_BLOCK, 7)?;
        let timing: [u8; 7] = raw_timing
            .try_into()
            .map_err(|_| DeviceError::Protocol("Unexpected timing block length".into()))?;
        Ok(parse_hdmi_timing(&timing, reg_1c, reg_24, reg_25))
    }

    /// Poll MCU interrupt / event register (`REG_GPIO_IN 0x0618` bit 12 active-low + MCU `0x2B`).
    /// Verified in `AVer330USB.sys` (`ProjectC875` GPIO 12, `CQLCodec_GetGPIOBitValue` reads `0x0618`
    /// when `0x0610` direction bit is 0, and `0x47d64` triggers when GPIO level == 0).
    /// Automatically acknowledges button single-click (`6`) by clearing `0x2B` to `0`.
    pub fn poll_mcu_event(&self, force_read: bool) -> Result<McuEvent> {
        if !force_read {
            let gpio_in = self.reg_read(REG_GPIO_IN).unwrap_or(0);
            // Active-low interrupt: bit 12 HIGH (1) means idle; LOW (0) means MCU event pending.
            if (gpio_in & GPIO_MCU_INT_BIT) != 0 {
                return Ok(McuEvent::None);
            }
        }
        let code = self.mcu_read(MCU_REG_EVENT, 1)?[0];
        let ev = McuEvent::from_raw(code);
        if matches!(ev, McuEvent::ButtonClick) {
            let _ = self.mcu_write_u8(MCU_REG_EVENT, 0x00);
        }
        Ok(ev)
    }

    /// Gather complete device identity and live signal information.
    pub fn read_info(&self) -> Result<DeviceInfo> {
        let chip_revision = self.read_chip_revision()?;
        let mcu_identity = self.read_mcu_identity().unwrap_or_else(|_| "unknown".into());
        let firmware_loaded = self.firmware_loaded().unwrap_or(false);
        let audio_source = self.get_audio_input().unwrap_or(AudioInput::Hdmi);
        let signal = self.read_signal().ok().flatten();
        Ok(DeviceInfo {
            bus: self.bus,
            address: self.address,
            chip_revision,
            chip_revision_ok: chip_revision == EXPECTED_CHIP_REV,
            mcu_identity,
            firmware_loaded,
            audio_source,
            signal,
        })
    }

    // -----------------------------------------------------------------------
    // RAM Boot Sequence (206 I2C Writes + DMA Upload + Verify + QSOS + Ping 0)
    // -----------------------------------------------------------------------

    pub fn boot_firmware(
        &self,
        fw: &VerifiedFirmware,
        mut progress: impl FnMut(&str),
    ) -> Result<()> {
        progress("Initializing QL202 memory windows & bridge registers");
        for &(reg, val) in &BRIDGE_MEM_WINDOW_REGS {
            self.reg_write(reg, val)?;
        }
        let _ = self.read_chip_revision()?;
        self.reg_write(REG_GPIO_DIR, 0)?;
        self.reg_write(REG_GPIO_OUT, 0)?;
        let _ = self.reg_read(REG_SYS_CTRL_50)?;
        self.reg_write(REG_SYS_CTRL_50, 0x0404)?;
        let _ = self.reg_read(REG_SYS_CTRL_50)?;
        self.reg_write(REG_SYS_CTRL_50, 0x0404)?;

        progress("Running official 206 I2C board initialization writes");
        for (chip, reg, val) in proto::build_init_i2c_writes() {
            self.i2c_write(chip, reg, val)?;
        }

        progress("Halting ARM core and preparing SDRAM windows");
        self.arm_set_run(false)?;
        self.reg_write(REG_TO_ARM_STATUS, 0)?;
        for &(reg, val) in &BRIDGE_MEM_WINDOW_REGS {
            self.reg_write(reg, val)?;
        }
        let _ = self.reg_read(REG_SYS_CTRL_50)?;
        self.reg_write(REG_SYS_CTRL_50, 0x0404)?;
        let _ = self.reg_read(REG_SYS_CTRL_50)?;
        self.reg_write(REG_SYS_CTRL_50, 0x0404)?;
        self.reg_write(REG_GPIO_DIR, 0)?;
        self.reg_write(REG_GPIO_OUT, 0)?;
        let _ = self.reg_read(REG_GPIO_DATA)?;
        self.reg_write(REG_GPIO_DATA, 0x07FF_0000)?;

        progress("Clearing Audio BSS & uploading qpaudfwusb.bin (DMA verified)");
        let mut bss_word = ARM_AUDIO_FW_WORD_BASE + (fw.audio_bytes.len() as u32 / 4);
        for words in ARM_AUDIO_BSS_CLEAR_CHUNKS {
            let zeros = vec![0u8; words as usize * 4];
            self.dma_write(bss_word, &zeros, false)?;
            bss_word += words;
        }
        self.dma_write(ARM_AUDIO_FW_WORD_BASE, &fw.audio_bytes, true)?;
        self.word_write(ARM_CFG_FIXED_MODE_WORD, 1)?;
        self.word_write(ARM_CFG_INT_MODE_WORD, 0)?;
        self.word_write(ARM_CFG_BASE_WORD + 1, 0)?;
        self.reg_write(REG_TO_ARM_STATUS, 0)?;

        progress("Uploading qpvidfwusb.bin (DMA verified)");
        self.dma_write(ARM_VIDEO_FW_WORD_BASE, &fw.video_bytes, true)?;

        progress("Starting ARM core and verifying QSOS signature");
        self.arm_set_run(true)?;
        self.reg_write(REG_TO_ARM_STATUS, 0)?;
        let _ = self.word_read_bytes(ARM_QSOS_MAGIC_WORD)?;
        let _ = self.word_read_bytes(ARM_QSOS_MAGIC_WORD + 1)?;
        let _ = self.word_read_bytes(ARM_CFG_BASE_WORD + 4)?;
        for _ in 0..2 {
            let _ = self.reg_read(REG_GPIO_DIR)?;
            self.reg_write(REG_GPIO_DIR, 0)?;
        }
        let _ = self.reg_read(REG_GPIO_DIR)?;
        let _ = self.reg_read(REG_GPIO_IN)?;

        if !self.firmware_loaded()? {
            return Err(DeviceError::Protocol(
                "QSOS firmware signature missing after ARM boot".into(),
            ));
        }

        progress("Verifying ARM mailbox ping (opcode 0x00)");
        self.ping_mailbox()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // ARM Mailbox Protocol (IRQ 0x800 bit 16 + 0x6C8/0x6CC/0x6FC)
    // -----------------------------------------------------------------------

    fn wait_to_arm_free(&self, tries: u32, step: Duration) -> Result<()> {
        for _ in 0..tries {
            if (self.reg_read(REG_TO_ARM_STATUS)? & 1) == 0 {
                return Ok(());
            }
            sleep(step);
        }
        Ok(())
    }

    pub fn mbox_send(&self, opcode: u32, task: u32, param0: u32, extra_params: &[u32]) -> Result<()> {
        self.wait_to_arm_free(200, Duration::from_millis(1))?;
        self.reg_write(REG_TO_ARM_PARAM0, param0)?;
        for (idx, &val) in extra_params.iter().enumerate() {
            let reg = REG_TO_ARM_PARAM0 - 4 - 4 * (idx as u32);
            self.reg_write(reg, val)?;
        }
        self.reg_write(REG_TO_ARM_STATUS, (task << 16) | 1)?;
        self.reg_write(REG_TO_ARM_MSG, (task << 16) | opcode)?;
        Ok(())
    }

    /// Ping ARM mailbox with opcode `0x00` and verify firmware consumes `0x6CC`.
    pub fn ping_mailbox(&self) -> Result<()> {
        self.mbox_send(MBOX_PING, 0, 0, &[])?;
        for _ in 0..100 {
            if (self.reg_read(REG_TO_ARM_STATUS)? & 1) == 0 {
                return Ok(());
            }
            sleep(Duration::from_millis(5));
        }
        Err(DeviceError::Protocol("ARM mailbox ping timed out".into()))
    }

    /// Set encoder digital audio input volume (`0..=15`) via `0x6F8` + mailbox `0x0A`.
    pub fn set_encoder_audio_volume(&self, vol: u8) -> Result<()> {
        self.mbox_send(MBOX_SET_AUDIO_INPUT_VOL, 0, vol.min(15) as u32, &[])
    }

    /// Update hardware MJPEG compression quality (`0..=15`) via ext-config sub-opcode `0x0B`.
    pub fn set_mjpeg_quality(&self, quality: u8) -> Result<()> {
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x0B, &[quality.min(15) as u32])
    }

    /// Acknowledge and release an ARM->Host mailbox message; returns frame buffer (`0x30`) for `0x40`.
    fn release_arm_message(&self, st: u32, msg_words: &[u32]) -> Result<()> {
        let (msg, p0, size_words, pts) = (msg_words[0], msg_words[1], msg_words[4], msg_words[5]);
        let kind = msg & 0xFFFF;
        let task = msg >> 16;

        self.reg_write(REG_FROM_ARM_STATUS, st & !1)?;
        if (st & 0x100) != 0 {
            let ack = if kind < 0x80 { MBOX_ACK_LOW } else { MBOX_ACK_HIGH };
            self.mbox_send(ack, task, 0, &[])?;
        }
        if (kind & 0xFF) == MSG_KIND_FRAME {
            self.mbox_send(
                MBOX_COMPLETE_BUFFER,
                task,
                p0 & 0xFFFF,
                &[
                    size_words,
                    pts & 0x7FFF_FFFF,
                    pts >> 31,
                    0,
                    (p0 >> 24) & 0xFF,
                ],
            )?;
        }
        Ok(())
    }

    /// Drain pending ARM->Host messages without copying payload data.
    pub fn drain_mailbox(&self) -> Result<usize> {
        let mut drained = 0;
        for _ in 0..2_000 {
            let irq = self.reg_read(REG_IRQ_STATUS).unwrap_or(0);
            if (irq & IRQ_ARM_MAILBOX_BIT) != 0 {
                let _ = self.reg_write(REG_IRQ_STATUS, irq);
            }
            let st = self.reg_read(REG_FROM_ARM_STATUS)?;
            if (st & 1) == 0 {
                break;
            }
            let words = self.reg_read_multi(REG_FROM_ARM_MSG, 6)?;
            self.release_arm_message(st, &words)?;
            drained += 1;
        }
        Ok(drained)
    }

    // -----------------------------------------------------------------------
    // Encoder Start / Poll / Stop (Both H.264+AAC and Hardware MJPEG)
    // -----------------------------------------------------------------------

    /// Configure audio routing, MCU RTC, and start the QL202 hardware encoder
    /// in either `H264Aac` (MPEG-TS) or `Mjpeg` (low-latency KVM) mode.
    pub fn start_encoder(
        &self,
        settings: &Settings,
        signal: Option<&HdmiSignal>,
    ) -> Result<EncoderRegisters> {
        settings.validate().map_err(DeviceError::Protocol)?;
        let regs = compute_encoder_registers(settings, signal);

        let drained = self.drain_mailbox()?;
        if drained > 0 {
            // An earlier interrupted session left active ARM frames pending; send StopEncoder + SystemClose
            // first so SystemOpen always starts from a clean firmware state.
            let _ = self.mbox_send(MBOX_STOP_ENCODER, 0, 0, &[]);
            sleep(Duration::from_millis(120));
            let _ = self.drain_mailbox();
            let _ = self.mbox_send(MBOX_SYSTEM_CLOSE, 0, 0, &[]);
            sleep(Duration::from_millis(120));
            let _ = self.drain_mailbox();
        }

        // Wake MCU & synchronize clock & audio input routing.
        let _ = self.mcu_write_u8(MCU_REG_WAKE, 0x01);
        let _ = self.sync_mcu_rtc();
        self.set_audio_input(settings.audio_input)?;
        if settings.audio_input == AudioInput::Jack {
            self.set_analog_input_volume(settings.analog_volume)?;
        }
        let _ = self.set_led_code(settings.led_mode.mcu_code().unwrap_or(0x05));

        // 1. Pre-open audio input volume (default 8)
        self.set_encoder_audio_volume(settings.encoder_volume)?;

        // 2. SystemOpen (0xF1) with codec function (H.264+AAC 0x80000011 or MJPEG 0x80000002)
        self.mbox_send(MBOX_SYSTEM_OPEN, 0, regs.enc_function, &[])?;
        let irq = self.reg_read(REG_IRQ_STATUS)?;
        if irq != 0 {
            let _ = self.reg_write(REG_IRQ_STATUS, irq & 0x07);
        }

        // 3. SystemLink (0xF2): video_in=0, video_out=1, audio_in=0, audio_out=1
        self.mbox_send(MBOX_SYSTEM_LINK, 0, ENC_SYSTEM_LINK_DEFAULT, &[])?;
        sleep(Duration::from_millis(20));

        // 4. Official 5 post-link I2C writes on chip 0x35
        for &(chip, reg, val) in &POST_LINK_I2C_WRITES {
            self.i2c_write(chip, reg, val)?;
        }

        // 5. Extended encoder configuration messages (opcode 0x10)
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x0F, &[0, 0])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x10, &[0, 0, 0])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x12, &[0])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x13, &[0x78, 0, 8])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x14, &[1, 19_000])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x16, &[1, 1])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x17, &[1])?;
        self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x02, &[0xF1F1_F1DA, 0xB6F1_F1B6])?;

        if let Some(q) = regs.mjpeg_quality {
            self.mbox_send(MBOX_SET_EXT_CONFIG, 0, 0x0B, &[q])?;
        }

        // 6. Write complete encoder parameter register block (0x6F8..0x6D0) and trigger StartEncoder (0x01)
        self.wait_to_arm_free(100, Duration::from_millis(5))?;
        self.reg_write(REG_TO_ARM_PARAM0, regs.sys_control)?;
        self.reg_write(REG_TO_ARM_PARAM1, regs.in_resolution)?;
        self.reg_write(REG_TO_ARM_PARAM2, regs.input_control)?;
        self.reg_write(REG_TO_ARM_PARAM3, regs.rate_control)?;
        self.reg_write(REG_TO_ARM_PARAM4, regs.vbr_bitrate)?;
        self.reg_write(REG_TO_ARM_PARAM5, regs.filter_control)?;
        self.reg_write(REG_TO_ARM_PARAM6, regs.gop_loop_filter)?;
        self.reg_write(REG_TO_ARM_PARAM8, regs.block_xfer_size)?;
        self.reg_write(REG_TO_ARM_PARAM7, regs.out_resolution)?;
        self.reg_write(REG_TO_ARM_AUDIO_CTRL, regs.audio_control)?;
        self.reg_write(REG_TO_ARM_AUDIO_EX, regs.audio_control_ex)?;
        self.reg_write(REG_TO_ARM_STATUS, 1)?;
        self.reg_write(REG_TO_ARM_MSG, MBOX_START_ENCODER)?;

        self.wait_to_arm_free(100, Duration::from_millis(5))?;
        // 7. Re-apply encoder digital audio volume after start
        self.set_encoder_audio_volume(settings.encoder_volume)?;

        Ok(regs)
    }

    /// Cleanly stop the hardware encoder (`0x02` StopEncoder -> drain -> `0xF3` SystemClose -> drain)
    /// so the GL310 can be restarted repeatedly without replugging USB.
    pub fn stop_encoder(&self) -> Result<()> {
        let _ = self.drain_mailbox();
        let _ = self.mbox_send(MBOX_STOP_ENCODER, 0, 0, &[]);
        sleep(Duration::from_millis(250));
        let _ = self.drain_mailbox();
        let _ = self.mbox_send(MBOX_SYSTEM_CLOSE, 0, 0, &[]);
        sleep(Duration::from_millis(250));
        let _ = self.drain_mailbox();
        let _ = self.set_led_code(0x04); // Solid idle ring
        Ok(())
    }

    /// IRQ-gated poll (`0x800` bit 16 + `0x6C8` bit 0) for the next captured media chunk.
    /// Reads ARM SDRAM payload BEFORE returning buffer `0x30` and applies `bswap32` to stream bytes only.
    pub fn poll_stream_chunk(&self, mode: CaptureMode) -> Result<Option<StreamChunk>> {
        // Gate on IRQ 0x800 bit 16 when raised, also checking 0x6C8 bit 0 directly to avoid missing
        // edge-triggered notifications during bursts.
        let irq = self.reg_read(REG_IRQ_STATUS)?;
        if (irq & IRQ_ARM_MAILBOX_BIT) != 0 {
            let _ = self.reg_write(REG_IRQ_STATUS, IRQ_ARM_MAILBOX_BIT);
        }

        let st = self.reg_read(REG_FROM_ARM_STATUS)?;
        if (st & 1) == 0 {
            return Ok(None);
        }

        let m = self.reg_read_multi(REG_FROM_ARM_MSG, 6)?;
        let msg = m[0];
        let p0 = m[1];
        let arm_addr = m[2];
        let size_words = m[4];
        let pts = m[5];

        let tag = p0 & 0xFFFF;
        let is_wanted = (msg & 0xFF) == MSG_KIND_FRAME
            && size_words > 0
            && size_words < (1 << 20)
            && match mode {
                CaptureMode::H264Aac => tag == STREAM_TAG_TS,
                // Hardware MJPEG emits TAG_VIDEO (0x81); also accept 0x83 if wrapped, validated by SOI/EOI downstream.
                CaptureMode::Mjpeg => tag == STREAM_TAG_VIDEO || tag == STREAM_TAG_TS,
            };

        let chunk = if is_wanted {
            let mut data = self.dma_read(arm_addr, size_words as usize * 4)?;
            // Hardware streams arrive word-reversed (big-endian 32-bit words); swap stream payload only.
            bswap32_in_place(&mut data);
            Some(StreamChunk { tag, pts, data })
        } else {
            None
        };

        // Always release mailbox & return buffer 0x30 even when chunk was ignored.
        self.release_arm_message(st, &m)?;
        Ok(chunk)
    }

    // -----------------------------------------------------------------------
    // Phase 1 Dry-Test (No HDMI Source Required)
    // -----------------------------------------------------------------------

    /// Execute complete no-HDMI hardware self-test:
    /// 1. Chip revision `0x0038` (`0x00010020`) & MCU `G310` identity
    /// 2. Safety guard self-test refusing `0x47` and `0x49`
    /// 3. Cycle all 4 LED ring patterns (`0x03`, `0x04`, `0x05`, `0x0B`)
    /// 4. Optional interactive physical button click wait
    /// 5. Full RAM firmware boot + DMA readback verify + `QSOS` + mailbox ping `0x00`
    pub fn run_dry_test(
        &self,
        fw: &VerifiedFirmware,
        wait_button_secs: u64,
        mut log: impl FnMut(&str),
    ) -> Result<DryTestReport> {
        let mut notes = Vec::new();

        let chip_revision = self.read_chip_revision()?;
        let chip_revision_ok = chip_revision == EXPECTED_CHIP_REV;
        log(&format!(
            "[1/5] Chip revision 0x0038 = {chip_revision:#010x} (expected {EXPECTED_CHIP_REV:#010x})"
        ));

        let mcu_identity = self.read_mcu_identity()?;
        log(&format!("[1/5] MCU identity (0x15/0x0A) = '{mcu_identity}'"));

        // Verify safety guard rejects permanent profile writes without touching USB.
        let g47 = self.mcu_write_u8(0x47, 0x00).is_err();
        let g49 = self.mcu_write_u8(0x49, 0x00).is_err();
        let safety_guard_ok = g47 && g49;
        log(&format!(
            "[2/5] Safety guard blocking MCU 0x47 & 0x49: {}",
            if safety_guard_ok { "PASS" } else { "FAIL" }
        ));

        log("[3/5] Cycling LED ring patterns (clockwise -> solid -> fade -> fast -> clockwise)...");
        for &(code, name) in &[
            (0x04u8, "clockwise"),
            (0x05u8, "solid"),
            (0x03u8, "fade"),
            (0x0Bu8, "fast"),
            (0x04u8, "clockwise"),
        ] {
            log(&format!("      LED 0x{code:02X} ({name})"));
            self.set_led_code(code)?;
            sleep(Duration::from_millis(600));
        }
        let leds_cycled = true;

        let (button_tested, button_clicked) = if wait_button_secs > 0 {
            log(&format!(
                "[4/5] Press the physical button on the GL310 within {wait_button_secs}s..."
            ));
            let _ = self.mcu_write_u8(MCU_REG_EVENT, 0x00);
            let deadline = Instant::now() + Duration::from_secs(wait_button_secs);
            let mut clicked = false;
            while Instant::now() < deadline {
                if matches!(self.poll_mcu_event(true)?, McuEvent::ButtonClick) {
                    log("      Button click detected (MCU 0x2B == 6)!");
                    clicked = true;
                    break;
                }
                sleep(Duration::from_millis(100));
            }
            if !clicked {
                notes.push("No physical button click received before timeout".into());
            }
            (true, clicked)
        } else {
            log("[4/5] Button wait skipped (--button-wait 0)");
            (false, false)
        };

        log("[5/5] Booting verified firmwares into RAM + DMA readback + QSOS + mailbox ping 0...");
        let already = self.firmware_loaded()?;
        if already {
            log("      QSOS already active in RAM — verifying mailbox ping 0x00...");
            self.ping_mailbox()?;
            notes.push("Firmware was already resident in RAM; mailbox ping verified".into());
        } else {
            self.boot_firmware(fw, |step| log(&format!("      {step}")))?;
        }

        let qsos_verified = self.firmware_loaded()?;
        let mailbox_ping_ok = self.ping_mailbox().is_ok();

        Ok(DryTestReport {
            usb_ok: true,
            chip_revision,
            chip_revision_ok,
            mcu_identity,
            leds_cycled,
            button_tested,
            button_clicked,
            firmware_booted: true,
            qsos_verified,
            mailbox_ping_ok,
            safety_guard_ok,
            notes,
        })
    }
}
