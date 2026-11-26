//! Stream processing (`bswap32`, MPEG-TS 188B deframer with PAT/PMT/IDR tracking & continuity
//! error statistics, hardware MJPEG SOI/EOI deframer) and non-blocking multi-subscriber `StreamHub`.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

pub const TS_PACKET_SIZE: usize = 188;
pub const TS_SYNC_BYTE: u8 = 0x47;

/// Swap bytes of every 32-bit word in place.
/// MUST be applied ONLY to DMA stream payload buffers (MPEG-TS / MJPEG), never to register reads.
pub fn bswap32_in_place(buf: &mut [u8]) {
    for chunk in buf.chunks_exact_mut(4) {
        chunk.reverse();
    }
}

/// Statistics tracked by the MPEG-TS parser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TsStats {
    pub packets: u64,
    pub bytes: u64,
    pub continuity_errors: u64,
    pub resyncs: u64,
    pub keyframes: u64,
}

/// 188-byte MPEG-TS deframer with sync recovery, PAT/PMT/IDR keyframe detection,
/// and per-PID continuity counter validation.
pub struct TsDeframer {
    buf: Vec<u8>,
    synced: bool,
    cc_by_pid: HashMap<u16, u8>,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    last_pat: Option<Vec<u8>>,
    last_pmt: Option<Vec<u8>>,
    stats: TsStats,
}

impl Default for TsDeframer {
    fn default() -> Self {
        Self::new()
    }
}

impl TsDeframer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
            synced: false,
            cc_by_pid: HashMap::new(),
            pmt_pid: None,
            video_pid: None,
            last_pat: None,
            last_pmt: None,
            stats: TsStats::default(),
        }
    }

    pub fn stats(&self) -> TsStats {
        self.stats
    }

    /// Return cached PAT + PMT packets so late-joining subscribers decode cleanly immediately.
    pub fn bootstrap_headers(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(ref pat) = self.last_pat {
            out.extend_from_slice(pat);
        }
        if let Some(ref pmt) = self.last_pmt {
            out.extend_from_slice(pmt);
        }
        out
    }

    /// Ingest raw byte-swapped MPEG-TS bytes, returning aligned 188-byte packet bytes and
    /// whether this batch contains a keyframe (IDR / random_access_indicator).
    pub fn push(&mut self, data: &[u8]) -> (Vec<u8>, bool) {
        self.buf.extend_from_slice(data);
        let mut aligned = Vec::with_capacity(data.len());
        let mut saw_idr = false;

        while self.buf.len() >= TS_PACKET_SIZE {
            if !self.synced || self.buf[0] != TS_SYNC_BYTE {
                if let Some(pos) = find_ts_sync(&self.buf) {
                    if pos > 0 {
                        self.buf.drain(..pos);
                        self.stats.resyncs += 1;
                    }
                    self.synced = true;
                } else {
                    // Keep last 187 bytes in case sync starts near tail
                    if self.buf.len() > TS_PACKET_SIZE {
                        let drop_n = self.buf.len() - TS_PACKET_SIZE;
                        self.buf.drain(..drop_n);
                    }
                    self.synced = false;
                    break;
                }
            }

            if self.buf.len() < TS_PACKET_SIZE {
                break;
            }
            if self.buf[0] != TS_SYNC_BYTE {
                self.synced = false;
                continue;
            }

            let pkt: [u8; TS_PACKET_SIZE] = self.buf[..TS_PACKET_SIZE].try_into().unwrap();
            self.buf.drain(..TS_PACKET_SIZE);

            self.stats.packets += 1;
            self.stats.bytes += TS_PACKET_SIZE as u64;

            let pid = (((pkt[1] & 0x1F) as u16) << 8) | (pkt[2] as u16);
            let pusi = (pkt[1] & 0x40) != 0;
            let afc = (pkt[3] >> 4) & 0x03;
            let cc = pkt[3] & 0x0F;

            // Check continuity counter for non-null packets carrying payload (afc == 1 or 3).
            if pid != 0x1FFF && (afc & 0x01) != 0 {
                let discontinuity = if (afc & 0x02) != 0 && pkt[4] > 0 {
                    (pkt[5] & 0x80) != 0
                } else {
                    false
                };
                if let Some(&prev) = self.cc_by_pid.get(&pid) {
                    let expected = (prev + 1) & 0x0F;
                    if !discontinuity && cc != expected && cc != prev {
                        self.stats.continuity_errors += 1;
                    }
                }
                self.cc_by_pid.insert(pid, cc);
            }

            // Capture PAT (PID 0x0000) and discover PMT PID & H.264 video PID.
            if pid == 0x0000 && pusi {
                self.last_pat = Some(pkt.to_vec());
                if let Some(pmt) = parse_pat_pmt_pid(&pkt) {
                    self.pmt_pid = Some(pmt);
                }
            } else if Some(pid) == self.pmt_pid && pusi {
                self.last_pmt = Some(pkt.to_vec());
                if let Some(vpid) = parse_pmt_video_pid(&pkt) {
                    self.video_pid = Some(vpid);
                }
            }

            let is_video_candidate = match self.video_pid {
                Some(vpid) => pid == vpid,
                None => pid != 0x0000 && Some(pid) != self.pmt_pid && pid != 0x1FFF,
            };
            if is_video_candidate && packet_has_idr_or_rai(&pkt) {
                saw_idr = true;
                self.stats.keyframes += 1;
            }

            aligned.extend_from_slice(&pkt);
        }

        (aligned, saw_idr)
    }
}

/// Require two consecutive 0x47 sync bytes separated by exactly 188 bytes so a random 0x47
/// inside leading garbage cannot cause a false sync lock.
fn find_ts_sync(buf: &[u8]) -> Option<usize> {
    if buf.len() <= TS_PACKET_SIZE {
        return None;
    }
    let last_start = buf.len() - TS_PACKET_SIZE - 1;
    (0..=last_start).find(|&i| buf[i] == TS_SYNC_BYTE && buf[i + TS_PACKET_SIZE] == TS_SYNC_BYTE)
}

fn payload_offset(pkt: &[u8; TS_PACKET_SIZE]) -> Option<usize> {
    let afc = (pkt[3] >> 4) & 0x03;
    match afc {
        1 => Some(4),
        3 => {
            let af_len = pkt[4] as usize;
            let off = 5 + af_len;
            if off < TS_PACKET_SIZE {
                Some(off)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_pat_pmt_pid(pkt: &[u8; TS_PACKET_SIZE]) -> Option<u16> {
    let off = payload_offset(pkt)?;
    let ptr = pkt[off] as usize;
    let sec = off + 1 + ptr;
    if sec + 12 >= TS_PACKET_SIZE || pkt[sec] != 0x00 {
        return None;
    }
    let section_len = (((pkt[sec + 1] & 0x0F) as usize) << 8) | (pkt[sec + 2] as usize);
    let entries_start = sec + 8;
    let entries_end = (sec + 3 + section_len).saturating_sub(4).min(TS_PACKET_SIZE);
    let mut pos = entries_start;
    while pos + 4 <= entries_end {
        let prog_num = ((pkt[pos] as u16) << 8) | (pkt[pos + 1] as u16);
        let pid = (((pkt[pos + 2] & 0x1F) as u16) << 8) | (pkt[pos + 3] as u16);
        if prog_num != 0 {
            return Some(pid);
        }
        pos += 4;
    }
    None
}

fn parse_pmt_video_pid(pkt: &[u8; TS_PACKET_SIZE]) -> Option<u16> {
    let off = payload_offset(pkt)?;
    let ptr = pkt[off] as usize;
    let sec = off + 1 + ptr;
    if sec + 12 >= TS_PACKET_SIZE || pkt[sec] != 0x02 {
        return None;
    }
    let section_len = (((pkt[sec + 1] & 0x0F) as usize) << 8) | (pkt[sec + 2] as usize);
    let prog_info_len = (((pkt[sec + 10] & 0x0F) as usize) << 8) | (pkt[sec + 11] as usize);
    let mut pos = sec + 12 + prog_info_len;
    let end = (sec + 3 + section_len).saturating_sub(4).min(TS_PACKET_SIZE);
    while pos + 5 <= end {
        let stream_type = pkt[pos];
        let elem_pid = (((pkt[pos + 1] & 0x1F) as u16) << 8) | (pkt[pos + 2] as u16);
        let es_info_len = (((pkt[pos + 3] & 0x0F) as usize) << 8) | (pkt[pos + 4] as usize);
        // 0x1B = H.264/AVC, 0x24 = HEVC, 0x02 = MPEG-2 video
        if matches!(stream_type, 0x1B | 0x24 | 0x02) {
            return Some(elem_pid);
        }
        pos += 5 + es_info_len;
    }
    None
}

fn packet_has_idr_or_rai(pkt: &[u8; TS_PACKET_SIZE]) -> bool {
    let afc = (pkt[3] >> 4) & 0x03;
    // Check MPEG-TS adaptation field random_access_indicator (bit 6 of flags byte).
    if (afc & 0x02) != 0 && pkt[4] > 0 && (pkt[5] & 0x40) != 0 {
        return true;
    }
    // Check H.264 Annex-B IDR (nal_unit_type == 5) or SPS (nal_unit_type == 7) NAL unit.
    let Some(off) = payload_offset(pkt) else {
        return false;
    };
    let payload = &pkt[off..];
    for w in payload.windows(4) {
        if w[0] == 0x00 && w[1] == 0x00 && w[2] == 0x01 {
            let nal_type = w[3] & 0x1F;
            if nal_type == 5 || nal_type == 7 {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Hardware MJPEG Deframer (Segment-aware SOI 0xFF 0xD8 .. EOI 0xFF 0xD9)
// ---------------------------------------------------------------------------

const MAX_MJPEG_BUFFER: usize = 4 * 1024 * 1024;

pub struct MjpegDeframer {
    buf: Vec<u8>,
    frames_emitted: u64,
}

impl Default for MjpegDeframer {
    fn default() -> Self {
        Self::new()
    }
}

impl MjpegDeframer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512 * 1024),
            frames_emitted: 0,
        }
    }

    pub fn frames_emitted(&self) -> u64 {
        self.frames_emitted
    }

    /// Ingest byte-swapped hardware MJPEG stream bytes and extract complete JPEG frames (`SOI..=EOI`).
    /// Walks length-prefixed JPEG segments (APPn/COM/DQT/DHT/SOF) so embedded EXIF thumbnails
    /// containing `0xFF 0xD9` cannot prematurely truncate a frame, and recovers cleanly if a
    /// dropped chunk causes a new `SOI` to appear before `EOI`.
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        if self.buf.len() > MAX_MJPEG_BUFFER {
            let trim = self.buf.len() - (MAX_MJPEG_BUFFER / 2);
            self.buf.drain(..trim);
        }

        let mut frames = Vec::new();
        loop {
            let Some(soi) = find_marker(&self.buf, 0xFF, 0xD8, 0) else {
                // Keep at most 1 trailing byte in case 0xFF sits at boundary
                if self.buf.len() > 1 {
                    let drop_n = self.buf.len() - 1;
                    self.buf.drain(..drop_n);
                }
                break;
            };
            if soi > 0 {
                self.buf.drain(..soi);
            }
            match scan_jpeg_frame_end(&self.buf) {
                JpegScan::Complete(end) => {
                    let frame = self.buf[..end].to_vec();
                    self.buf.drain(..end);
                    if frame.len() >= 128 {
                        self.frames_emitted += 1;
                        frames.push(frame);
                    }
                }
                JpegScan::ResyncAt(next_soi) => {
                    // Truncated frame followed by a fresh SOI — drop broken prefix and retry.
                    self.buf.drain(..next_soi);
                }
                JpegScan::NeedMore => break,
            }
        }
        frames
    }
}

enum JpegScan {
    Complete(usize),
    ResyncAt(usize),
    NeedMore,
}

/// Walk JPEG segments starting at `buf[0..2] == [0xFF, 0xD8]` to locate the genuine frame `EOI`.
fn scan_jpeg_frame_end(buf: &[u8]) -> JpegScan {
    let mut pos = 2usize;
    let len = buf.len();
    let mut in_entropy_scan = false;

    while pos < len {
        if in_entropy_scan {
            // Inside SOS entropy-coded data: 0xFF 0x00 is byte-stuffed literal 0xFF;
            // 0xFF 0xD0..=0xD7 are RST markers; 0xFF 0xD9 is genuine EOI; 0xFF 0xD8 means corrupted/new frame.
            let Some(ff_rel) = buf[pos..].iter().position(|&b| b == 0xFF) else {
                return JpegScan::NeedMore;
            };
            pos += ff_rel;
            // Skip fill 0xFF bytes
            while pos + 1 < len && buf[pos + 1] == 0xFF {
                pos += 1;
            }
            if pos + 1 >= len {
                return JpegScan::NeedMore;
            }
            let marker = buf[pos + 1];
            match marker {
                0x00 | 0xD0..=0xD7 => {
                    pos += 2;
                }
                0xD9 => return JpegScan::Complete(pos + 2),
                0xD8 => return JpegScan::ResyncAt(pos),
                // Another marker after scan (e.g. DNL / multi-scan SOS): leave entropy mode
                _ => {
                    in_entropy_scan = false;
                }
            }
        } else {
            // Expect marker prefix 0xFF; if payload wasn't structured (e.g. synthetic test frames),
            // fall back cleanly to raw EOI/SOI search.
            if buf[pos] != 0xFF {
                let mut i = pos;
                while i + 1 < len {
                    if buf[i] == 0xFF {
                        if buf[i + 1] == 0xD9 {
                            return JpegScan::Complete(i + 2);
                        }
                        if buf[i + 1] == 0xD8 {
                            return JpegScan::ResyncAt(i);
                        }
                    }
                    i += 1;
                }
                return JpegScan::NeedMore;
            }
            while pos + 1 < len && buf[pos + 1] == 0xFF {
                pos += 1;
            }
            if pos + 1 >= len {
                return JpegScan::NeedMore;
            }
            let marker = buf[pos + 1];
            match marker {
                0xD9 => return JpegScan::Complete(pos + 2),
                0xD8 => return JpegScan::ResyncAt(pos),
                0x00 | 0x01 | 0xD0..=0xD7 => {
                    pos += 2;
                }
                0xDA => {
                    // Start of Scan: 2-byte header length followed by entropy-coded bitstream
                    if pos + 4 > len {
                        return JpegScan::NeedMore;
                    }
                    let seg_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
                    if seg_len < 2 {
                        pos += 2;
                    } else if pos + 2 + seg_len > len {
                        return JpegScan::NeedMore;
                    } else {
                        pos += 2 + seg_len;
                        in_entropy_scan = true;
                    }
                }
                _ => {
                    // Length-prefixed segment (APPn, COM, DQT, DHT, SOFn, DRI, ...)
                    if pos + 4 > len {
                        return JpegScan::NeedMore;
                    }
                    let seg_len = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
                    if seg_len < 2 {
                        pos += 2;
                    } else if pos + 2 + seg_len > len {
                        return JpegScan::NeedMore;
                    } else {
                        pos += 2 + seg_len;
                    }
                }
            }
        }
    }
    JpegScan::NeedMore
}

fn find_marker(buf: &[u8], b0: u8, b1: u8, start: usize) -> Option<usize> {
    if buf.len() < start + 2 {
        return None;
    }
    (start..buf.len() - 1).find(|&i| buf[i] == b0 && buf[i + 1] == b1)
}

// ---------------------------------------------------------------------------
// Non-Blocking Multi-Subscriber StreamHub
// ---------------------------------------------------------------------------

struct TsSubscriber {
    tx: SyncSender<Arc<Vec<u8>>>,
    needs_keyframe: bool,
    skipped_for_idr: u64,
    dropped_chunks: u64,
}

struct JpegSubscriber {
    tx: SyncSender<Arc<Vec<u8>>>,
}

#[derive(Default)]
struct HubInner {
    ts_subs: Vec<TsSubscriber>,
    jpeg_subs: Vec<JpegSubscriber>,
    last_pat_pmt: Vec<u8>,
    last_jpeg: Option<Arc<Vec<u8>>>,
    ts_stats: TsStats,
    mjpeg_frames: u64,
}

/// Thread-safe, non-blocking fan-out hub distributing MPEG-TS chunks and JPEG frames
/// to V4L2, PipeWire, HTTP `/stream.ts`, HTTP `/mjpeg`, `/snapshot.jpg`, SRT, and recorder sinks.
#[derive(Clone, Default)]
pub struct StreamHub {
    inner: Arc<Mutex<HubInner>>,
}

impl StreamHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to MPEG-TS chunks. If `wait_idr` is true, delivery starts cleanly on PAT+PMT+IDR.
    pub fn subscribe_ts(&self, capacity: usize, wait_idr: bool) -> Receiver<Arc<Vec<u8>>> {
        let (tx, rx) = mpsc::sync_channel(capacity.max(4));
        let mut g = self.inner.lock().unwrap();
        g.ts_subs.push(TsSubscriber {
            tx,
            needs_keyframe: wait_idr,
            skipped_for_idr: 0,
            dropped_chunks: 0,
        });
        rx
    }

    /// Subscribe to complete JPEG frames (either hardware MJPEG or preview transcoder output).
    pub fn subscribe_jpeg(&self, capacity: usize) -> Receiver<Arc<Vec<u8>>> {
        let (tx, rx) = mpsc::sync_channel(capacity.max(2));
        let mut g = self.inner.lock().unwrap();
        if let Some(ref latest) = g.last_jpeg {
            let _ = tx.try_send(Arc::clone(latest));
        }
        g.jpeg_subs.push(JpegSubscriber { tx });
        rx
    }

    /// Publish aligned MPEG-TS bytes; drops slow subscribers' packets instead of stalling USB reads.
    pub fn publish_ts(
        &self,
        chunk: Vec<u8>,
        has_idr: bool,
        pat_pmt: &[u8],
        stats: TsStats,
    ) {
        if chunk.is_empty() {
            return;
        }
        let payload = Arc::new(chunk);
        let mut g = self.inner.lock().unwrap();
        g.ts_stats = stats;
        if !pat_pmt.is_empty() {
            g.last_pat_pmt.clear();
            g.last_pat_pmt.extend_from_slice(pat_pmt);
        }
        let header_arc = if !g.last_pat_pmt.is_empty() {
            Some(Arc::new(g.last_pat_pmt.clone()))
        } else {
            None
        };

        g.ts_subs.retain_mut(|sub| {
            if sub.needs_keyframe {
                if !has_idr {
                    sub.skipped_for_idr += 1;
                    // Safety valve: if >120 chunks arrive without an IDR marker (e.g. split NAL header
                    // or unusual interlaced/VESA bitstream), unlock delivery anyway so ffmpeg/V4L2/preview
                    // can resync natively instead of stalling forever with 0 frames.
                    if sub.skipped_for_idr < 120 {
                        return true;
                    }
                }
                if let Some(ref hdr) = header_arc {
                    let _ = sub.tx.try_send(Arc::clone(hdr));
                }
                sub.needs_keyframe = false;
            }
            match sub.tx.try_send(Arc::clone(&payload)) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    sub.dropped_chunks += 1;
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    /// Publish a complete JPEG frame (used by hardware MJPEG mode and H.264 preview snapshot bridge).
    pub fn publish_jpeg(&self, frame: Vec<u8>) {
        if frame.is_empty() {
            return;
        }
        let arc = Arc::new(frame);
        let mut g = self.inner.lock().unwrap();
        g.last_jpeg = Some(Arc::clone(&arc));
        g.mjpeg_frames += 1;
        g.jpeg_subs.retain(|sub| match sub.tx.try_send(Arc::clone(&arc)) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    pub fn latest_jpeg(&self) -> Option<Arc<Vec<u8>>> {
        self.inner.lock().unwrap().last_jpeg.clone()
    }

    pub fn ts_stats(&self) -> TsStats {
        self.inner.lock().unwrap().ts_stats
    }

    pub fn mjpeg_frames(&self) -> u64 {
        self.inner.lock().unwrap().mjpeg_frames
    }

    pub fn clear_snapshot(&self) {
        let mut g = self.inner.lock().unwrap();
        g.last_jpeg = None;
    }

    /// Clear cached JPEG snapshot, PAT/PMT bootstrap headers, and TS counters when restarting or
    /// stopping capture so newly subscribed outputs never receive stale pre-restart headers or frames.
    pub fn reset_cache(&self) {
        let mut g = self.inner.lock().unwrap();
        g.last_jpeg = None;
        g.last_pat_pmt.clear();
        g.ts_stats = TsStats::default();
        g.mjpeg_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bswap32_swaps_only_full_words() {
        let mut data = vec![0x47, 0x40, 0x00, 0x10, 0x01, 0x02, 0x03, 0x04];
        bswap32_in_place(&mut data);
        assert_eq!(data, vec![0x10, 0x00, 0x40, 0x47, 0x04, 0x03, 0x02, 0x01]);
        bswap32_in_place(&mut data);
        assert_eq!(data, vec![0x47, 0x40, 0x00, 0x10, 0x01, 0x02, 0x03, 0x04]);
    }

    fn make_ts_packet(pid: u16, pusi: bool, cc: u8, rai: bool, payload: &[u8]) -> [u8; 188] {
        let mut pkt = [0xFFu8; 188];
        pkt[0] = TS_SYNC_BYTE;
        pkt[1] = ((pid >> 8) as u8 & 0x1F) | if pusi { 0x40 } else { 0 };
        pkt[2] = (pid & 0xFF) as u8;
        if rai {
            pkt[3] = 0x30 | (cc & 0x0F); // adaptation + payload
            pkt[4] = 1;
            pkt[5] = 0x40; // random_access_indicator
            let n = payload.len().min(182);
            pkt[6..6 + n].copy_from_slice(&payload[..n]);
        } else {
            pkt[3] = 0x10 | (cc & 0x0F);
            let n = payload.len().min(184);
            pkt[4..4 + n].copy_from_slice(&payload[..n]);
        }
        pkt
    }

    #[test]
    fn ts_deframer_resyncs_detects_idr_and_counts_cc_errors() {
        let mut d = TsDeframer::new();
        let mut raw = vec![0xDE, 0xAD, 0xBE]; // leading garbage
        raw.extend_from_slice(&make_ts_packet(0x100, true, 0, true, &[0, 0, 1, 0x65]));
        raw.extend_from_slice(&make_ts_packet(0x100, false, 1, false, &[1, 2, 3]));
        raw.extend_from_slice(&make_ts_packet(0x100, false, 4, false, &[4, 5, 6])); // gap: expected 2

        let (aligned, idr) = d.push(&raw);
        assert_eq!(aligned.len(), 3 * 188);
        assert!(idr);
        assert_eq!(d.stats().packets, 3);
        assert_eq!(d.stats().resyncs, 1);
        assert_eq!(d.stats().continuity_errors, 1);
        assert_eq!(d.stats().keyframes, 1);
    }

    #[test]
    fn mjpeg_deframer_extracts_soi_to_eoi_frames() {
        let mut d = MjpegDeframer::new();
        let mut stream = vec![0x00, 0x11, 0xFF, 0xD8];
        stream.extend(vec![0xAA; 200]);
        stream.extend_from_slice(&[0xFF, 0xD9, 0x00, 0x00, 0xFF, 0xD8]);
        stream.extend(vec![0xBB; 150]);
        stream.extend_from_slice(&[0xFF, 0xD9]);

        let f1 = d.push(&stream[..100]);
        assert!(f1.is_empty());
        let f2 = d.push(&stream[100..]);
        assert_eq!(f2.len(), 2);
        assert!(f2[0].starts_with(&[0xFF, 0xD8]) && f2[0].ends_with(&[0xFF, 0xD9]));
        assert!(f2[1].starts_with(&[0xFF, 0xD8]) && f2[1].ends_with(&[0xFF, 0xD9]));
        assert_eq!(d.frames_emitted(), 2);
    }

    #[test]
    fn stream_hub_waits_for_idr_and_never_blocks() {
        let hub = StreamHub::new();
        let rx_idr = hub.subscribe_ts(4, true);
        let rx_any = hub.subscribe_ts(4, false);

        hub.publish_ts(vec![0x47; 188], false, &[], TsStats::default());
        assert!(rx_idr.try_recv().is_err());
        assert!(rx_any.try_recv().is_ok());

        hub.publish_ts(vec![0x47; 188], true, &[0x47; 188], TsStats::default());
        assert!(rx_idr.try_recv().is_ok()); // header
        assert!(rx_idr.try_recv().is_ok()); // IDR chunk
    }

    #[test]
    fn ts_deframer_ignores_lone_sync_in_garbage_and_false_idr_in_audio_pid() {
        let mut d = TsDeframer::new();
        // Lone 0x47 in 200 bytes of garbage must not emit false packets
        let mut garbage = vec![0xAA; 200];
        garbage[10] = 0x47;
        let (aligned0, _) = d.push(&garbage);
        assert!(aligned0.is_empty());

        // Valid PAT pointing program 1 -> PMT PID 0x1000
        let pat_payload: [u8; 17] = [
            0x00, // pointer
            0x00, // table_id PAT
            0xB0, 0x0D, // section_length = 13
            0x00, 0x01, 0xC1, 0x00, 0x00, // ts_id, ver, sec
            0x00, 0x01, // program 1
            0xF0, 0x00, // PMT PID 0x1000
            0x00, 0x00, 0x00, 0x00, // CRC
        ];
        // Valid PMT: H.264 (0x1B) on PID 0x0100, AAC (0x0F) on PID 0x0101
        let pmt_payload: [u8; 27] = [
            0x00, // pointer
            0x02, // table_id PMT
            0xB0, 0x17, // section_length = 23
            0x00, 0x01, 0xC1, 0x00, 0x00,
            0xE1, 0x00, // PCR PID 0x100
            0xF0, 0x00, // program_info_length = 0
            0x1B, 0xE1, 0x00, 0xF0, 0x00, // H.264 PID 0x100
            0x0F, 0xE1, 0x01, 0xF0, 0x00, // AAC PID 0x101
            0x00, 0x00, 0x00, 0x00, // CRC
        ];
        let mut stream = Vec::new();
        stream.extend_from_slice(&make_ts_packet(0x0000, true, 0, false, &pat_payload));
        stream.extend_from_slice(&make_ts_packet(0x1000, true, 0, false, &pmt_payload));
        // Audio packet containing accidental 00 00 01 65 must NOT be treated as video IDR!
        stream.extend_from_slice(&make_ts_packet(0x0101, true, 0, false, &[0x00, 0x00, 0x01, 0x65]));
        let (aligned1, idr1) = d.push(&stream);
        assert_eq!(aligned1.len(), 3 * 188);
        assert!(!idr1);

        // Actual H.264 video packet with IDR MUST trigger saw_idr
        let vid_pkt = make_ts_packet(0x0100, true, 0, false, &[0x00, 0x00, 0x01, 0x65]);
        let (_, idr2) = d.push(&vid_pkt);
        assert!(idr2);
    }

    #[test]
    fn mjpeg_deframer_handles_app1_embedded_eoi_and_truncated_resync() {
        let mut d = MjpegDeframer::new();
        // Build a structured JPEG with an APP1 (0xFF 0xE1) segment containing an embedded 0xFF 0xD9
        let mut jpg = vec![0xFF, 0xD8];
        // APP1 segment: length = 10 (2 len bytes + 8 payload bytes containing FF D9)
        jpg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x0A, b'E', b'x', b'i', b'f', 0xFF, 0xD9, 0x00, 0x00]);
        // SOS segment: length = 6 (2 len + 4 header bytes)
        jpg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x06, 0x01, 0x01, 0x00, 0x00]);
        // Entropy payload with byte-stuffed 0xFF 0x00
        jpg.extend(vec![0x42; 140]);
        jpg.extend_from_slice(&[0xFF, 0x00, 0x33, 0xFF, 0xD9]);

        // Prepend a truncated frame (SOI + partial data, missing EOI) followed by valid jpg
        let mut combined = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x04, 0x00, 0x00, 0x11, 0x22];
        combined.extend_from_slice(&jpg);

        let frames = d.push(&combined);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], jpg);
    }
}
