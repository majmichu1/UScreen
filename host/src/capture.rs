use anyhow::{Context, Result};
use bytes::Bytes;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

const RECONNECT_DELAY_MS: u64 = 2000;
const FIFO_PATH: &str = "/tmp/uscreen_capture.fifo";

// H.264 NAL unit types
const NAL_TYPE_NON_IDR: u8 = 1;
const NAL_TYPE_IDR: u8 = 5;
const NAL_TYPE_AUD: u8 = 9;
const NAL_TYPE_SPS: u8 = 7;
const NAL_TYPE_PPS: u8 = 8;

/// One H.264 access unit, tagged so the stream server can drop frames
/// safely (resume only at an IDR).
#[derive(Clone)]
pub struct VideoPacket {
    pub data: Bytes,
    pub is_idr: bool,
}

/// Settings that can change at runtime (from the GUI or the tablet app).
/// A change restarts the encoder; an fps or resolution change also restarts
/// the helper (the EDID is regenerated for the new mode).
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderSettings {
    pub encoder: String,
    pub fps: u32,
    pub bitrate: u32,
    pub width: u32,
    pub height: u32,
}

pub struct CaptureConfig {
    pub helper_path: PathBuf,
    /// Explicit EDID override; None = generate one for the configured mode
    pub edid_path: Option<PathBuf>,
    pub encoder: String,
    pub fps: u32,
    pub bitrate: u32,
    pub width: u32,
    pub height: u32,
    pub capture_mode: String,  // "evdi" or "screencap"
    pub screen_name: String,   // KWin output name for screencap mode
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            helper_path: PathBuf::from("host/evdi/evdi_helper"),
            edid_path: None,
            encoder: String::from("h264_nvenc"),
            fps: 60,
            bitrate: 20000,
            width: 2960,
            height: 1848,
            capture_mode: String::from("evdi"),
            screen_name: String::from("DVI-I-9"),
        }
    }
}

pub struct CaptureManager {
    config: CaptureConfig,
    helper_child: Option<Child>,
    encoder_child: Option<Child>,
    codec_config: Arc<Mutex<Option<Bytes>>>,
}

impl CaptureManager {
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            helper_child: None,
            encoder_child: None,
            codec_config: Arc::new(Mutex::new(None)),
        }
    }

    pub fn codec_config_arc(&self) -> Arc<Mutex<Option<Bytes>>> {
        self.codec_config.clone()
    }

    fn ensure_fifo(path: &str) -> Result<()> {
        // Check if FIFO already exists and is usable
        match std::fs::metadata(path) {
            Ok(meta) if meta.file_type().is_fifo() => {
                // Check permissions instead of opening (opening a FIFO for write would block)
                if meta.permissions().mode() & 0o200 != 0 {
                    return Ok(());
                }
                // Not writable — need to recreate
                let _ = std::fs::remove_file(path);
            }
            Ok(_) => {
                // Regular file in the way
                let _ = std::fs::remove_file(path);
            }
            Err(_) => {} // Doesn't exist, will create below
        }

        let status = std::process::Command::new("mkfifo")
            .arg("-m")
            .arg("0666")
            .arg(path)
            .status()
            .context("Failed to run mkfifo")?;
        if !status.success() {
            anyhow::bail!("mkfifo failed");
        }
        Ok(())
    }

    /// Find the EVDI DVI output via kscreen-doctor, enable it, and set position.
    /// Retries up to ~3 seconds with 200ms gaps so KWin has time to discover the new output.
    async fn enable_evdi_display() {
        let mut evdi_id: Option<u32> = None;
        for attempt in 0..15 {
            let output = tokio::process::Command::new("kscreen-doctor")
                .arg("-o")
                .output()
                .await;
            if let Ok(o) = output {
                let stdout = String::from_utf8_lossy(&o.stdout);
                for line in stdout.lines() {
                    let line = line.trim();
                    if let Some(rest) = line.strip_prefix("Output:") {
                        let parts: Vec<&str> = rest.trim().split_whitespace().collect();
                        if parts.len() >= 2 && parts.get(1).copied().unwrap_or("").contains("DVI") {
                            evdi_id = parts[0].parse::<u32>().ok();
                            break;
                        }
                    }
                }
            }
            if evdi_id.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if let Some(id) = evdi_id {
            info!("Enabling EVDI display (output.{})", id);
            for cmd in &[
                format!("output.{}.mode.1", id),
                format!("output.{}.enable", id),
                format!("output.{}.position.1920.0", id),
            ] {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let r = tokio::process::Command::new("kscreen-doctor")
                    .arg(cmd)
                    .output()
                    .await;
                match r {
                    Ok(o) if o.status.success() => info!("kscreen-doctor {}: ok", cmd),
                    _ => warn!("kscreen-doctor {}: failed (may be harmless)", cmd),
                }
            }
        } else {
            warn!("No EVDI output found after 3s");
        }
    }

    async fn start_helper(&mut self) -> Result<()> {
        Self::ensure_fifo(FIFO_PATH)?;

        let edid_path = match &self.config.edid_path {
            Some(p) => p.clone(),
            None => crate::edid::ensure_edid(self.config.width, self.config.height, 60)?,
        };

        let mut cmd = Command::new(&self.config.helper_path);
        cmd.args(["--edid", &edid_path.to_string_lossy()]);
        cmd.args(["--fps", &self.config.fps.to_string()]);

        // In screencap mode, the helper only creates the display (no FIFO writes)
        if self.config.capture_mode == "evdi" {
            cmd.args(["--capture-fifo", FIFO_PATH]);
        } else {
            info!("Helper started in screencap mode (no FIFO capture)");
        }

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn().context("Failed to spawn evdi-helper")?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("No stdout from helper"))?;

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        loop {
            match lines.next_line().await {
                Ok(Some(l)) => {
                    if let Some(card) = l.strip_prefix("EVDI_CONNECTED card") {
                        let _: u32 = card.parse()?;
                        info!("Helper connected on card{}", card);
                        break;
                    }
                }
                Ok(None) => {
                    anyhow::bail!("evdi-helper exited prematurely");
                }
                Err(e) => {
                    anyhow::bail!("evdi-helper stdout error: {}", e);
                }
            }
        }

        self.helper_child = Some(child);
        Ok(())
    }

    async fn start_encoder(&mut self) -> Result<()> {
        // Accept the old gstreamer-style name as an alias
        let encoder = if self.config.encoder == "vaapih264enc" {
            "h264_vaapi".to_string()
        } else {
            self.config.encoder.clone()
        };
        let w = self.config.width;
        let h = self.config.height;
        let fps = self.config.fps;
        let bitrate = self.config.bitrate;
        // Keyframe every second: enough for fast client joins without
        // burning the whole bitrate budget on IDR frames.
        let gop = fps.max(1);

        let mut encoder_args: Vec<String> = vec!["-hide_banner".into()];

        if encoder == "h264_vaapi" {
            encoder_args.extend_from_slice(&[
                "-vaapi_device".into(),
                "/dev/dri/renderD128".into(),
            ]);
        }

        encoder_args.extend_from_slice(&[
            "-fflags".into(),
            "nobuffer".into(),
            "-flags".into(),
            "low_delay".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "bgr0".into(),  // EVDI outputs XRGB8888 = B-G-R-X byte order on little-endian
            "-s".into(),
            format!("{}x{}", w, h),
            "-framerate".into(),
            fps.to_string(),
            "-i".into(),
            FIFO_PATH.into(),
        ]);

        if encoder == "h264_vaapi" {
            encoder_args.extend_from_slice(&[
                "-vf".into(),
                "format=nv12,hwupload".into(),
            ]);
        }

        encoder_args.extend_from_slice(&["-c:v".into(), encoder.clone()]);

        if encoder == "h264_nvenc" {
            let bitrate_m = bitrate as f64 / 1000.0;
            encoder_args.extend_from_slice(&[
                "-preset".into(),
                "p1".into(),
                "-tune".into(),
                "ull".into(),
                "-zerolatency".into(),
                "1".into(),
                "-delay".into(),
                "0".into(),
                "-bf".into(),
                "0".into(),
                "-rc-lookahead".into(),
                "0".into(),
                "-color_primaries".into(),
                "bt709".into(),
                "-color_trc".into(),
                "bt709".into(),
                "-colorspace".into(),
                "bt709".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                "20".into(),
                "-b:v".into(),
                format!("{:.1}M", bitrate_m),
                "-maxrate".into(),
                format!("{:.1}M", bitrate_m),
                "-bufsize".into(),
                "1.0M".into(),  // small VBV = lower latency
                "-g".into(),
                gop.to_string(),
                "-forced-idr".into(),
                "1".into(),
            ]);
        } else if encoder == "h264_vaapi" {
            encoder_args.extend_from_slice(&[
                "-rc_mode".into(),
                "VBR".into(),
                "-b:v".into(),
                format!("{}k", bitrate),
                "-maxrate".into(),
                format!("{}k", bitrate),
                "-bf".into(),
                "0".into(),
                "-g".into(),
                gop.to_string(),
                "-idr_interval".into(),
                "1".into(),
            ]);
        } else if encoder == "libx264" {
            encoder_args.extend_from_slice(&[
                "-preset".into(),
                "ultrafast".into(),
                "-tune".into(),
                "zerolatency".into(),
                "-crf".into(),
                "20".into(),
                "-maxrate".into(),
                format!("{}k", bitrate),
                "-bufsize".into(),
                format!("{}k", bitrate),
                "-g".into(),
                gop.to_string(),
                "-x264-params".into(),
                "scenecut=0".into(),
            ]);
        } else {
            anyhow::bail!(
                "Unknown encoder: {}. Use h264_nvenc, h264_vaapi, or libx264",
                encoder
            );
        }

        encoder_args.extend_from_slice(&["-f".into(), "h264".into(), "pipe:1".into()]);

        let mut cmd = Command::new("ffmpeg");
        cmd.args(&encoder_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let child = cmd.spawn().context("Failed to spawn ffmpeg encoder")?;
        info!("Encoder started (PID: {})", child.id().unwrap_or(0));
        self.encoder_child = Some(child);
        Ok(())
    }

    pub async fn stream_frames(
        &mut self,
        tx: broadcast::Sender<VideoPacket>,
        mut settings_rx: watch::Receiver<EncoderSettings>,
    ) -> Result<()> {
        let mut screencap_task: Option<tokio::task::JoinHandle<()>> = None;
        // Exponential backoff: a crash-looping helper floods KWin with
        // display hotplug events, which can wedge the whole desktop.
        let mut backoff_ms: u64 = RECONNECT_DELAY_MS;
        let mut pipeline_started_at = Instant::now();

        loop {
            // Apply the latest runtime settings before (re)starting anything
            {
                let s = settings_rx.borrow_and_update().clone();
                let needs_helper_restart = (s.fps != self.config.fps
                    || s.width != self.config.width
                    || s.height != self.config.height)
                    && self.helper_child.is_some();
                if needs_helper_restart {
                    // fps is baked into the helper's pacing, and the
                    // resolution into the EDID — restart with a fresh EDID
                    info!(
                        "Display mode change: {}x{}@{} → {}x{}@{}",
                        self.config.width, self.config.height, self.config.fps,
                        s.width, s.height, s.fps
                    );
                    if let Some(mut h) = self.helper_child.take() {
                        let _ = h.start_kill();
                    }
                    // Give the compositor a moment to process the unplug
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                self.config.encoder = s.encoder;
                self.config.fps = s.fps;
                self.config.bitrate = s.bitrate;
                self.config.width = s.width;
                self.config.height = s.height;
            }

            // Start evdi-helper if not running
            if self.helper_child.is_none() {
                if let Err(e) = self.start_helper().await {
                    error!("Failed to start helper: {}. Retrying in {}ms...", e, backoff_ms);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                    continue;
                }
                pipeline_started_at = Instant::now();
            }

            // Start screencap in screencap mode
            if self.config.capture_mode == "screencap" && screencap_task.is_none() {
                Self::ensure_fifo(FIFO_PATH)?;
                let screen_name = self.config.screen_name.clone();
                let fifo = FIFO_PATH.to_string();
                screencap_task = Some(tokio::spawn(async move {
                    let cap = crate::screencap::ScreenCapture::new(&screen_name);
                    if let Err(e) = cap.run(&fifo).await {
                        error!("ScreenCapture error: {}", e);
                    }
                }));
                info!("ScreenCapture task spawned for output '{}'", self.config.screen_name);
            }

            // In evdi mode, enable the display via kscreen-doctor so KWin
            // actively renders to it (enabling evdi_grab_pixels).
            if self.config.capture_mode == "evdi" {
                Self::enable_evdi_display().await;
            }

            // Start encoder if not running
            if self.encoder_child.is_none() {
                if let Err(e) = self.start_encoder().await {
                    error!("Failed to start encoder: {}. Retrying in {}ms...", e, backoff_ms);
                    if let Some(mut h) = self.helper_child.take() {
                        let _ = h.start_kill();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                    continue;
                }
            }

            let child = self.encoder_child.as_mut().unwrap();
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Encoder has no stdout"))?;

            let mut settings_changed = false;
            tokio::select! {
                result = Self::read_loop(&mut stdout, tx.clone(), self.codec_config.clone()) => {
                    match result {
                        Ok(_) => info!("Encoder process exited"),
                        Err(e) => warn!("Encoder error: {}. Restarting...", e),
                    }
                }
                _ = settings_rx.changed() => {
                    info!("Settings changed — restarting encoder");
                    settings_changed = true;
                }
            }

            // A pipeline that ran for a while was healthy — reset the backoff.
            // A pipeline that died within seconds is crash-looping — back off.
            if pipeline_started_at.elapsed().as_secs() >= 30 {
                backoff_ms = RECONNECT_DELAY_MS;
            } else if !settings_changed {
                backoff_ms = (backoff_ms * 2).min(30_000);
            }

            // Clean up and retry. On a settings change, keep the helper alive
            // (unless fps changed — handled at the top of the loop) so the
            // virtual display doesn't flicker off.
            if !settings_changed {
                if let Some(mut h) = self.helper_child.take() {
                    let _ = h.start_kill();
                }
            }
            if let Some(mut e) = self.encoder_child.take() {
                let _ = e.start_kill();
            }
            // Cancel screencap task
            if let Some(task) = screencap_task.take() {
                task.abort();
            }
            // Reset codec config so it gets re-extracted on restart
            if let Ok(mut config) = self.codec_config.lock() {
                *config = None;
            }
            if !settings_changed {
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }
        }
    }

    /// Find all NAL start codes in a buffer and return their positions.
    fn find_start_codes(data: &[u8]) -> Vec<usize> {
        let mut starts = Vec::new();
        let mut i = 0;
        while i < data.len().saturating_sub(3) {
            // Check for 4-byte start code: 00 00 00 01
            if i + 3 < data.len()
                && data[i] == 0
                && data[i + 1] == 0
                && data[i + 2] == 0
                && data[i + 3] == 1
            {
                starts.push(i);
                i += 4;
            }
            // Check for 3-byte start code: 00 00 01
            else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                starts.push(i);
                i += 3;
            } else {
                i += 1;
            }
        }
        starts
    }

    fn nal_header_offset(data: &[u8], start: usize) -> Option<usize> {
        if start + 4 < data.len()
            && data[start] == 0
            && data[start + 1] == 0
            && data[start + 2] == 0
            && data[start + 3] == 1
        {
            Some(start + 4)
        } else if start + 3 < data.len()
            && data[start] == 0
            && data[start + 1] == 0
            && data[start + 2] == 1
        {
            Some(start + 3)
        } else {
            None
        }
    }

    async fn read_loop(
        stdout: &mut (impl tokio::io::AsyncRead + Unpin),
        tx: broadcast::Sender<VideoPacket>,
        codec_config: Arc<Mutex<Option<Bytes>>>,
    ) -> Result<()> {
        let mut buf = vec![0u8; 512 * 1024];
        let mut total: u64 = 0;
        let mut frames: u64 = 0;
        let mut last_log = Instant::now();
        let mut packetizer = H264AnnexBPacketizer::new();
        let mut config_extracted = codec_config.lock().ok().and_then(|g| g.clone()).is_some();

        loop {
            let n = stdout
                .read(&mut buf)
                .await
                .context("Read error from encoder")?;

            if n == 0 {
                for data in packetizer.finish() {
                    if tx.receiver_count() > 0 {
                        let _ = tx.send(data);
                    }
                }
                return Ok(());
            }

            total += n as u64;

            let chunk = &buf[..n];
            let access_units = packetizer.push(chunk);

            if !config_extracted {
                if let Some(config) = packetizer.codec_config() {
                    info!("Extracted codec config (SPS+PPS): {} bytes", config.len());
                    if let Ok(mut cc) = codec_config.lock() {
                        *cc = Some(config);
                    }
                    config_extracted = true;
                } else if total > 1024 * 1024 {
                    warn!("Could not find SPS/PPS in first 1MB of stream");
                    config_extracted = true;
                }
            }

            for data in access_units {
                frames += 1;
                if tx.receiver_count() > 0 {
                    let _ = tx.send(data);
                }
            }

            if last_log.elapsed().as_secs() >= 5 {
                let elapsed = last_log.elapsed().as_secs_f64();
                let mbps = if elapsed > 0.0 {
                    (total as f64 / elapsed) / 1_048_576.0
                } else {
                    0.0
                };
                let kbps = mbps * 8.0 * 1024.0;
                info!(
                    "Encoder: {} access units in {:.1}s, {:.1} MB/s ({:.0} kbps)",
                    frames, elapsed, mbps, kbps
                );
                frames = 0;
                total = 0;
                last_log = Instant::now();
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.helper_child.take() {
            let _ = child.start_kill();
        }
        if let Some(mut child) = self.encoder_child.take() {
            let _ = child.start_kill();
        }
    }
}

impl Drop for CaptureManager {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_file(FIFO_PATH);
    }
}

struct H264AnnexBPacketizer {
    buffer: Vec<u8>,
    pending_access_unit: Vec<u8>,
    pending_has_vcl: bool,
    pending_has_idr: bool,
    config: Vec<u8>,
    has_sps: bool,
    has_pps: bool,
}

impl H264AnnexBPacketizer {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            pending_access_unit: Vec::new(),
            pending_has_vcl: false,
            pending_has_idr: false,
            config: Vec::new(),
            has_sps: false,
            has_pps: false,
        }
    }

    fn push(&mut self, data: &[u8]) -> Vec<VideoPacket> {
        self.buffer.extend_from_slice(data);
        self.process_complete_nals(false)
    }

    fn finish(&mut self) -> Vec<VideoPacket> {
        let mut out = self.process_complete_nals(true);
        if let Some(access_unit) = self.take_pending_access_unit() {
            out.push(access_unit);
        }
        out
    }

    fn codec_config(&self) -> Option<Bytes> {
        if self.has_sps && self.has_pps {
            Some(Bytes::copy_from_slice(&self.config))
        } else {
            None
        }
    }

    fn process_complete_nals(&mut self, flush: bool) -> Vec<VideoPacket> {
        let mut out = Vec::new();
        let starts = CaptureManager::find_start_codes(&self.buffer);

        if starts.is_empty() {
            if self.buffer.len() > 3 {
                let keep_from = self.buffer.len() - 3;
                self.buffer.drain(..keep_from);
            }
            return out;
        }

        if starts[0] > 0 {
            self.buffer.drain(..starts[0]);
        }

        let starts = CaptureManager::find_start_codes(&self.buffer);
        if starts.len() < 2 && !flush {
            return out;
        }

        let nal_count = if flush {
            starts.len()
        } else {
            starts.len() - 1
        };
        for idx in 0..nal_count {
            let start = starts[idx];
            let end = starts.get(idx + 1).copied().unwrap_or(self.buffer.len());
            let nal = self.buffer[start..end].to_vec();
            self.process_nal(&nal, &mut out);
        }

        let drain_to = if flush {
            self.buffer.len()
        } else {
            starts[starts.len() - 1]
        };
        self.buffer.drain(..drain_to);
        out
    }

    fn process_nal(&mut self, nal: &[u8], out: &mut Vec<VideoPacket>) {
        let Some(header_offset) = CaptureManager::nal_header_offset(nal, 0) else {
            return;
        };
        if header_offset >= nal.len() {
            return;
        }

        let nal_type = nal[header_offset] & 0x1f;
        match nal_type {
            NAL_TYPE_SPS => {
                self.has_sps = true;
                self.config.extend_from_slice(nal);
            }
            NAL_TYPE_PPS => {
                self.has_pps = true;
                self.config.extend_from_slice(nal);
            }
            NAL_TYPE_AUD => {
                if let Some(access_unit) = self.take_pending_access_unit() {
                    out.push(access_unit);
                }
                self.pending_access_unit.extend_from_slice(nal);
            }
            NAL_TYPE_NON_IDR..=NAL_TYPE_IDR => {
                if self.pending_has_vcl && Self::first_mb_in_slice(nal, header_offset) == Some(0) {
                    if let Some(access_unit) = self.take_pending_access_unit() {
                        out.push(access_unit);
                    }
                }
                if nal_type == NAL_TYPE_IDR {
                    self.pending_has_idr = true;
                }
                self.pending_access_unit.extend_from_slice(nal);
                self.pending_has_vcl = true;
            }
            _ => {
                self.pending_access_unit.extend_from_slice(nal);
            }
        }
    }

    fn take_pending_access_unit(&mut self) -> Option<VideoPacket> {
        if !self.pending_has_vcl || self.pending_access_unit.is_empty() {
            self.pending_access_unit.clear();
            self.pending_has_vcl = false;
            self.pending_has_idr = false;
            return None;
        }

        let was_idr = self.pending_has_idr;
        self.pending_has_vcl = false;
        self.pending_has_idr = false;

        let au_data = std::mem::take(&mut self.pending_access_unit);

        // Prepend SPS/PPS to IDR frames so the decoder can always decode them,
        // even if it missed the initial config packet or reconnected mid-stream.
        let data = if was_idr && self.has_sps && self.has_pps {
            let mut full = Vec::with_capacity(self.config.len() + au_data.len());
            full.extend_from_slice(&self.config);
            full.extend_from_slice(&au_data);
            Bytes::from(full)
        } else {
            Bytes::from(au_data)
        };
        Some(VideoPacket {
            data,
            is_idr: was_idr,
        })
    }

    fn first_mb_in_slice(nal: &[u8], header_offset: usize) -> Option<u32> {
        let payload = nal.get(header_offset + 1..)?;
        ExpGolombReader::new(payload).read_ue()
    }
}

struct ExpGolombReader<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u8,
}

impl<'a> ExpGolombReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            bit: 0,
        }
    }

    fn read_ue(mut self) -> Option<u32> {
        let mut leading_zero_bits = 0u32;
        while self.read_bit()? == 0 {
            leading_zero_bits += 1;
            if leading_zero_bits > 31 {
                return None;
            }
        }

        let mut value = 1u32.checked_shl(leading_zero_bits)?;
        for shift in (0..leading_zero_bits).rev() {
            value |= (self.read_bit()? as u32) << shift;
        }
        Some(value - 1)
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.byte)?;
        let value = (byte >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.byte += 1;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(nal_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 1, nal_type];
        data.extend_from_slice(payload);
        data
    }

    #[test]
    fn packetizer_handles_start_code_split_across_reads() {
        let mut packetizer = H264AnnexBPacketizer::new();
        assert!(packetizer.push(&[0, 0]).is_empty());
        assert!(packetizer.push(&[0, 1, NAL_TYPE_IDR, 0x80]).is_empty());

        let out = packetizer.finish();
        assert_eq!(out.len(), 1);
        // No SPS/PPS seen, so IDR is emitted as-is
        assert_eq!(&out[0].data[..], &[0, 0, 0, 1, NAL_TYPE_IDR, 0x80]);
    }

    #[test]
    fn packetizer_splits_multiple_access_units_in_one_buffer() {
        let mut packetizer = H264AnnexBPacketizer::new();
        let mut data = nal(NAL_TYPE_AUD, &[0x10]);
        data.extend_from_slice(&nal(NAL_TYPE_IDR, &[0x80]));
        data.extend_from_slice(&nal(NAL_TYPE_AUD, &[0x10]));
        data.extend_from_slice(&nal(NAL_TYPE_NON_IDR, &[0x80]));

        let out = packetizer.push(&data);
        assert_eq!(out.len(), 1);
        // No SPS/PPS seen, so IDR AU emitted as-is
        assert_eq!(
            &out[0].data[..],
            &[
                0,
                0,
                0,
                1,
                NAL_TYPE_AUD,
                0x10,
                0,
                0,
                0,
                1,
                NAL_TYPE_IDR,
                0x80
            ]
        );

        let out = packetizer.finish();
        assert_eq!(out.len(), 1);
        assert_eq!(
            &out[0].data[..],
            &[
                0,
                0,
                0,
                1,
                NAL_TYPE_AUD,
                0x10,
                0,
                0,
                0,
                1,
                NAL_TYPE_NON_IDR,
                0x80
            ]
        );
    }

    #[test]
    fn packetizer_prepends_sps_pps_to_idr() {
        let mut packetizer = H264AnnexBPacketizer::new();
        let mut data = nal(NAL_TYPE_SPS, &[0x64, 0x00]);
        data.extend_from_slice(&nal(NAL_TYPE_PPS, &[0xac]));
        data.extend_from_slice(&nal(NAL_TYPE_IDR, &[0x80]));

        assert!(packetizer.push(&data).is_empty());
        let config = packetizer.codec_config().expect("codec config");
        assert_eq!(
            &config[..],
            &[
                0,
                0,
                0,
                1,
                NAL_TYPE_SPS,
                0x64,
                0x00,
                0,
                0,
                0,
                1,
                NAL_TYPE_PPS,
                0xac
            ]
        );

        let out = packetizer.finish();
        assert_eq!(out.len(), 1);
        // IDR frame should now have SPS+PPS prepended
        assert_eq!(
            &out[0].data[..],
            &[
                // SPS
                0, 0, 0, 1, NAL_TYPE_SPS, 0x64, 0x00,
                // PPS
                0, 0, 0, 1, NAL_TYPE_PPS, 0xac,
                // IDR
                0, 0, 0, 1, NAL_TYPE_IDR, 0x80
            ]
        );
    }

    #[test]
    fn packetizer_does_not_emit_partial_nals() {
        let mut packetizer = H264AnnexBPacketizer::new();
        let first = nal(NAL_TYPE_IDR, &[0x80, 0x11, 0x22]);

        assert!(packetizer.push(&first[..4]).is_empty());
        assert!(packetizer.push(&first[4..]).is_empty());

        let mut second = nal(NAL_TYPE_NON_IDR, &[0x80]);
        let out = packetizer.push(&second[..3]);
        assert!(out.is_empty());

        second.drain(..3);
        let out = packetizer.push(&second);
        assert!(out.is_empty());

        let out = packetizer.finish();
        assert_eq!(out.len(), 2);
        // No SPS/PPS seen, IDR emitted as-is
        assert_eq!(&out[0].data[..], &first[..]);
    }
}
