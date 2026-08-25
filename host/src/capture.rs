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
pub const FIFO_PATH: &str = "/tmp/uscreen_capture.fifo";


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
    /// Monotonically increasing per encoder run. Echoed back by the tablet once
    /// the frame is on screen, which is how end-to-end latency is measured.
    pub seq: u32,
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
    /// Constant-quality target; see `config::DEFAULT_QUALITY`.
    pub quality: u32,
    /// Physical panel size for the generated EDID, in millimetres.
    pub width_mm: u32,
    pub height_mm: u32,
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
    pub quality: u32,
    pub width_mm: u32,
    pub height_mm: u32,
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
            quality: crate::config::DEFAULT_QUALITY,
            width_mm: crate::edid::DEFAULT_WIDTH_MM,
            height_mm: crate::edid::DEFAULT_HEIGHT_MM,
        }
    }
}

/// The display mode KWin actually negotiated on the virtual output, as
/// reported by the helper's `MODE_CHANGED` line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectedMode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

pub struct CaptureManager {
    config: CaptureConfig,
    helper_child: Option<Child>,
    encoder_child: Option<Child>,
    codec_config: Arc<Mutex<Option<Bytes>>>,
    /// `None` until the running helper reports a mode.
    ///
    /// The compositor is free to pick a mode other than the EDID's preferred
    /// one (it remembers per-output settings across sessions). When it does,
    /// the helper produces frames of one size while ffmpeg is told another,
    /// and every frame comes out skewed — a failure that used to be invisible
    /// because the helper's stdout was closed right after the handshake.
    mode_tx: watch::Sender<Option<DetectedMode>>,
    mode_rx: watch::Receiver<Option<DetectedMode>>,
    helper_stdout_task: Option<tokio::task::JoinHandle<()>>,
    /// DRM card index the running helper attached to, used to address the
    /// virtual output unambiguously.
    helper_card: Option<u32>,
    latency: crate::latency::LatencyTracker,
}

impl CaptureManager {
    pub fn new(config: CaptureConfig) -> Self {
        let (mode_tx, mode_rx) = watch::channel(None);
        Self {
            config,
            helper_child: None,
            encoder_child: None,
            codec_config: Arc::new(Mutex::new(None)),
            mode_tx,
            mode_rx,
            helper_stdout_task: None,
            helper_card: None,
            latency: crate::latency::LatencyTracker::new(),
        }
    }

    /// Shared with the input server, which receives the tablet's render
    /// acknowledgements and closes the measurement loop.
    pub fn latency_tracker(&self) -> crate::latency::LatencyTracker {
        self.latency.clone()
    }

    pub fn codec_config_arc(&self) -> Arc<Mutex<Option<Bytes>>> {
        self.codec_config.clone()
    }

    /// Frame size the encoder must be configured for: whatever the compositor
    /// actually negotiated, falling back to the requested mode until the
    /// helper has reported one.
    fn active_mode(&self) -> (u32, u32) {
        match *self.mode_rx.borrow() {
            Some(m) if m.width > 0 && m.height > 0 => (m.width, m.height),
            _ => (self.config.width, self.config.height),
        }
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

    /// Enable the EVDI output and place it to the right of every other screen.
    ///
    /// The output is identified by the DRM connector names sysfs reports for
    /// EVDI cards, not by whether the name happens to contain "DVI": a real DVI
    /// monitor on a dock produces exactly the same name pattern, and acting on
    /// it would enable and move the user's physical screen instead of ours.
    ///
    /// The position is derived at runtime from the existing display geometry so
    /// it works regardless of the laptop's screen resolution or scaling factor.
    async fn enable_evdi_display(card: Option<u32>) {
        let evdi_names: Vec<String> = crate::vdisplay::evdi_connectors()
            .into_iter()
            .filter(|c| card.is_none_or(|want| c.card == want))
            .map(|c| c.name)
            .collect();
        if evdi_names.is_empty() {
            warn!("No EVDI connector found in sysfs — cannot enable the virtual display");
            return;
        }

        // Retry: KWin may not have registered the new EVDI device yet.
        // Use -j (JSON) rather than the plain "-o" text listing: newer
        // kscreen-doctor versions emit ANSI color codes in "-o" output
        // unconditionally, even when piped to a non-tty, which broke the
        // old line-based "Output:"/"Geometry:" parser (it silently matched
        // nothing, since every line actually starts with an escape code).
        for attempt in 0..15 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let Ok(o) = tokio::process::Command::new("kscreen-doctor")
                .arg("-j")
                .output()
                .await
            else {
                continue;
            };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) else {
                continue;
            };
            let Some(outputs) = v.get("outputs").and_then(|o| o.as_array()) else {
                continue;
            };

            let mut evdi: Option<(u32, bool, i64)> = None; // (id, enabled, x)
            let mut right_edge: i64 = 0;

            for out in outputs {
                let id = out.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let name = out.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let enabled = out.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                let x = out.pointer("/pos/x").and_then(|v| v.as_i64()).unwrap_or(0);

                if evdi_names.iter().any(|n| n == name) {
                    evdi = Some((id, enabled, x));
                } else if enabled {
                    // `pos` is already in logical (scaled) coordinates,
                    // matching what "position.X,Y" expects, but `size`
                    // is the raw physical mode resolution — divide by
                    // `scale` to get the logical width before adding,
                    // or the computed edge lands far past the actual
                    // screen (e.g. a 2560px-wide physical panel at
                    // 1.35x scale is only ~1897 logical px wide).
                    let w = out.pointer("/size/width").and_then(|v| v.as_i64()).unwrap_or(0);
                    let scale = out.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
                    let logical_w = if scale > 0.0 {
                        (w as f64 / scale).round() as i64
                    } else {
                        w
                    };
                    right_edge = right_edge.max(x + logical_w);
                }
            }

            let Some((id, enabled, x)) = evdi else {
                continue;
            };

            // Already where it belongs. Re-issuing the command would make KWin
            // reconfigure every output, flickering the whole desktop — which is
            // what used to happen on each pass of the capture loop.
            if enabled && x == right_edge {
                return;
            }

            info!(
                "Enabling EVDI output.{} at position ({}, 0)",
                id, right_edge
            );
            // All args in one kscreen-doctor call so KDE applies them atomically.
            // Position syntax is "X,Y" (comma-separated).
            let r = tokio::process::Command::new("kscreen-doctor")
                .arg(format!("output.{}.enable", id))
                .arg(format!("output.{}.position.{},0", id, right_edge))
                .output()
                .await;
            match r {
                Ok(o) if o.status.success() => info!("kscreen-doctor enable+position: ok"),
                Ok(o) => warn!(
                    "kscreen-doctor failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => warn!("kscreen-doctor error: {}", e),
            }
            return;
        }

        warn!("EVDI output did not appear in kscreen-doctor within 3s");
    }

    /// Turn the virtual output back off so its windows return to the real
    /// screens. Without this the desktop keeps a monitor nobody can see after
    /// the tablet is unplugged, and windows left there are effectively lost.
    pub async fn disable_evdi_display(card: Option<u32>) {
        let evdi_names: Vec<String> = crate::vdisplay::evdi_connectors()
            .into_iter()
            .filter(|c| card.is_none_or(|want| c.card == want))
            .map(|c| c.name)
            .collect();
        if evdi_names.is_empty() {
            return;
        }
        let Ok(o) = tokio::process::Command::new("kscreen-doctor")
            .arg("-j")
            .output()
            .await
        else {
            return;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) else {
            return;
        };
        let Some(outputs) = v.get("outputs").and_then(|o| o.as_array()) else {
            return;
        };
        for out in outputs {
            let name = out.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !evdi_names.iter().any(|n| n == name) {
                continue;
            }
            if !out.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                continue;
            }
            let id = out.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            info!("Disabling EVDI output.{} ({})", id, name);
            let _ = tokio::process::Command::new("kscreen-doctor")
                .arg(format!("output.{}.disable", id))
                .output()
                .await;
        }
    }

    async fn start_helper(&mut self) -> Result<()> {
        Self::ensure_fifo(FIFO_PATH)?;

        // Kill any stray helper from a previous run before spawning a new one.
        // kill_on_drop only fires on a graceful exit; if the daemon was
        // SIGKILLed, pkill'd, or crashed, its helper is orphaned and keeps
        // writing full frames into the shared FIFO. Several such orphans
        // interleave their output, which the encoder reads as a single
        // stream — producing torn, banded frames mixing several captures.
        // -x matches the process name exactly so it never hits this daemon
        // (whose own command line contains the helper path via --helper).
        let killed_helper = Command::new("pkill")
            .args(["-x", "evdi_helper"])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        // A stray ffmpeg reading the same FIFO is just as bad as a stray
        // helper writing it — two readers/writers on one pipe interleave at
        // pipe granularity and corrupt frames. Match on the FIFO path so we
        // never touch an unrelated ffmpeg invocation.
        let killed_ffmpeg = Command::new("pkill")
            .args(["-f", &format!("ffmpeg.*{}", FIFO_PATH)])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if killed_helper || killed_ffmpeg {
            warn!("Killed stray evdi_helper/ffmpeg process(es) before starting");
            // Give the kernel a moment to release the EVDI device(s) and
            // drop the old FIFO write end before we open a fresh one.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }

        // EDID 1.4 pixel-clock field is 16-bit (max 655 MHz).
        // 2960×1848 @120 Hz needs ~706 MHz which overflows.
        // Cap the EDID at 90 Hz so KDE can render at 90 fps; the helper
        // captures at the configured fps independently via clock_nanosleep.
        let edid_fps = self.config.fps.min(90);
        let edid_path = match &self.config.edid_path {
            Some(p) => p.clone(),
            None => crate::edid::ensure_edid_sized(
                self.config.width,
                self.config.height,
                edid_fps,
                self.config.width_mm,
                self.config.height_mm,
            )?,
        };

        let mut cmd = Command::new(&self.config.helper_path);
        cmd.args(["--edid", &edid_path.to_string_lossy()]);
        cmd.args(["--fps", &self.config.fps.to_string()]);

        cmd.args(["--capture-fifo", FIFO_PATH]);

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

        let card: u32;
        loop {
            match lines.next_line().await {
                Ok(Some(l)) => {
                    if let Some(rest) = l.strip_prefix("EVDI_CONNECTED card") {
                        card = rest.trim().parse()?;
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

        self.helper_card = Some(card);
        // A fresh helper has not negotiated a mode yet.
        let _ = self.mode_tx.send(None);

        // Keep draining stdout for the life of the helper. Dropping the reader
        // here (as this code used to) closes the pipe, so every later
        // MODE_CHANGED line dies with EPIPE and the host never learns what the
        // compositor really picked.
        if let Some(task) = self.helper_stdout_task.take() {
            task.abort();
        }
        let mode_tx = self.mode_tx.clone();
        self.helper_stdout_task = Some(tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                let Some(rest) = line.strip_prefix("MODE_CHANGED ") else {
                    continue;
                };
                let mut parts = rest.split_whitespace();
                let parsed = (|| {
                    Some(DetectedMode {
                        width: parts.next()?.parse().ok()?,
                        height: parts.next()?.parse().ok()?,
                        refresh: parts.next()?.parse().ok()?,
                    })
                })();
                let Some(mode) = parsed else {
                    warn!("Unparseable MODE_CHANGED from helper: {}", rest);
                    continue;
                };
                if mode.width == 0 || mode.height == 0 {
                    continue;
                }
                info!(
                    "Compositor negotiated {}x{}@{}Hz on the virtual output",
                    mode.width, mode.height, mode.refresh
                );
                let _ = mode_tx.send(Some(mode));
            }
        }));

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
        // Always the mode the compositor really drives, never the one we asked
        // for: a mismatch here shows up as a permanently skewed picture.
        let (w, h) = self.active_mode();
        if (w, h) != (self.config.width, self.config.height) {
            warn!(
                "Encoding at {}x{} — compositor did not honour the requested {}x{}",
                w, h, self.config.width, self.config.height
            );
        }
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
            // The helper converts the BGRA framebuffer to NV12 before the
            // FIFO: 1.5 bytes/px instead of 4, so the raw-frame copies that
            // bottleneck the pipeline shrink 2.7x and NVENC takes it directly.
            "nv12".into(),
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
            // bufsize = 1 frame of bits: keeps VBV under 1-frame delay.
            let bufsize_k = (bitrate / fps.max(1)).max(200);
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
                "-multipass".into(),
                "0".into(),
                "-color_primaries".into(),
                "bt709".into(),
                "-color_trc".into(),
                "bt709".into(),
                "-colorspace".into(),
                "bt709".into(),
                // Helper emits BT.709 limited-range NV12; tag it so the
                // decoder expands the range correctly. Full range was tried
                // and reverted — see the note in evdi_helper.c.
                "-color_range".into(),
                "tv".into(),
                // Constant-quality VBR, not CBR.
                //
                // CBR pads every frame to hit the target rate, so a completely
                // motionless desktop still pushed the full bitrate down the USB
                // link — measured at 7.5 MB/s with nothing moving on screen.
                // That traffic buys nothing and leaves no headroom for the
                // moments that do need it. With `-b:v 0` plus `-cq`, NVENC
                // spends bits only where the picture actually changes and
                // `-maxrate` still caps the bursts.
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                self.config.quality.to_string(),
                "-b:v".into(),
                "0".into(),
                "-maxrate".into(),
                format!("{:.1}M", bitrate_m),
                "-bufsize".into(),
                format!("{}k", bufsize_k),
                "-g".into(),
                gop.to_string(),
                "-forced-idr".into(),
                "1".into(),
            ]);
        } else if encoder == "h264_vaapi" {
            // Constant quality, for the same reason as NVENC above: a static
            // desktop should cost nothing, and the bitrate is only a ceiling.
            encoder_args.extend_from_slice(&[
                "-rc_mode".into(),
                "CQP".into(),
                "-qp".into(),
                self.config.quality.to_string(),
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
            let bufsize_k = (bitrate * 2 / fps.max(1)).max(200);
            encoder_args.extend_from_slice(&[
                "-preset".into(),
                "ultrafast".into(),
                "-tune".into(),
                "zerolatency".into(),
                "-crf".into(),
                self.config.quality.to_string(),
                "-maxrate".into(),
                format!("{}k", bitrate),
                "-bufsize".into(),
                format!("{}k", bufsize_k),
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
        mut tablet_rx: watch::Receiver<bool>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        // Exponential backoff: a crash-looping helper floods KWin with
        // display hotplug events, which can wedge the whole desktop.
        let mut backoff_ms: u64 = RECONNECT_DELAY_MS;
        let mut pipeline_started_at = Instant::now();
        let mut mode_rx = self.mode_rx.clone();
        // Frame size the running encoder was configured for, so a later mode
        // change can be detected as a mismatch rather than silently skewing.
        let mut encoder_mode: Option<(u32, u32)> = None;

        loop {
            // Apply the latest runtime settings before (re)starting anything
            {
                let s = settings_rx.borrow_and_update().clone();
                // The physical size is baked into the EDID alongside the mode,
                // so a change there needs a fresh helper too.
                let needs_helper_restart = (s.fps != self.config.fps
                    || s.width != self.config.width
                    || s.height != self.config.height
                    || s.width_mm != self.config.width_mm
                    || s.height_mm != self.config.height_mm)
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
                self.config.quality = s.quality;
                self.config.width_mm = s.width_mm;
                self.config.height_mm = s.height_mm;
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

            // Enable the display via kscreen-doctor so KWin actively renders
            // to it (which is what makes evdi_grab_pixels produce anything).
            //
            // Only while a tablet is actually attached: enabling it
            // unconditionally puts a monitor on the desktop that nobody can
            // see, and KDE happily moves windows onto it. The tablet_rx branch
            // below enables it the moment one is plugged in.
            if *tablet_rx.borrow() {
                Self::enable_evdi_display(self.helper_card).await;
            }

            // Wait (briefly) for the helper to report the mode the compositor
            // settled on before configuring ffmpeg's frame size. Guessing here
            // and getting it wrong yields a skewed picture for the whole
            // session, so a short wait is cheap insurance.
            //
            // Skipped with no tablet attached: the output is disabled then, so
            // no mode is ever reported and the wait would just add three
            // seconds and a warning to every daemon start.
            if *tablet_rx.borrow() && self.mode_rx.borrow().is_none() {
                let mut wait_rx = self.mode_rx.clone();
                if tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    wait_rx.changed(),
                )
                .await
                .is_err()
                {
                    warn!(
                        "Compositor reported no mode within 3s — encoding at the requested {}x{}",
                        self.config.width, self.config.height
                    );
                }
            }
            mode_rx.borrow_and_update();

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
                encoder_mode = Some(self.active_mode());
            }

            let child = self.encoder_child.as_mut().unwrap();
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Encoder has no stdout"))?;

            let mut settings_changed = false;
            // Distinct from `settings_changed`: the mode moved under us, so the
            // encoder must be rebuilt but the helper and the virtual display
            // are fine and must not be torn down.
            let mut mode_changed = false;
            // Events that need no restart at all — the encoder keeps running and
            // its stdout is handed back to the next iteration.
            let mut resume_same_encoder = false;
            let card = self.helper_card;
            tokio::select! {
                result = Self::read_loop(
                    &mut stdout,
                    tx.clone(),
                    self.codec_config.clone(),
                    self.latency.clone(),
                ) => {
                    match result {
                        Ok(_) => info!("Encoder process exited"),
                        Err(e) => warn!("Encoder error: {}. Restarting...", e),
                    }
                }
                _ = settings_rx.changed() => {
                    info!("Settings changed — restarting encoder");
                    settings_changed = true;
                }
                _ = mode_rx.changed() => {
                    let now = self.active_mode();
                    if Some(now) == encoder_mode {
                        // KWin re-applying the same mode. Nothing to do.
                        resume_same_encoder = true;
                    } else {
                        info!(
                            "Virtual output changed to {}x{} — restarting encoder to match",
                            now.0, now.1
                        );
                        mode_changed = true;
                    }
                }
                _ = tablet_rx.changed() => {
                    // Follow the tablet: an unplugged tablet must not leave a
                    // monitor behind that nobody can see, with windows stranded
                    // on it. The encoder itself is unaffected either way.
                    let present = *tablet_rx.borrow();
                    if present {
                        info!("Tablet present — enabling the virtual display");
                        Self::enable_evdi_display(card).await;
                    } else {
                        info!("Tablet gone — disabling the virtual display");
                        Self::disable_evdi_display(card).await;
                    }
                    resume_same_encoder = true;
                }
                _ = shutdown_rx.changed() => {
                    info!("Shutdown requested — tearing down the capture pipeline");
                    // Close our read end rather than handing it back. Nothing
                    // drains it during shutdown, so ffmpeg would block writing
                    // into a full pipe and never reach its signal handling —
                    // costing a wasted 1.5s SIGTERM timeout on every stop.
                    // Closed, it gets EPIPE and exits at once.
                    drop(stdout);
                    Self::disable_evdi_display(card).await;
                    self.shutdown().await;
                    return Ok(());
                }
            }

            if resume_same_encoder {
                // Hand stdout back so the next iteration keeps reading from the
                // same still-healthy encoder instead of tearing it down.
                if let Some(child) = self.encoder_child.as_mut() {
                    child.stdout = Some(stdout);
                }
                continue;
            }

            // A pipeline that ran for a while was healthy — reset the backoff.
            // A pipeline that died within seconds is crash-looping — back off.
            if pipeline_started_at.elapsed().as_secs() >= 30 {
                backoff_ms = RECONNECT_DELAY_MS;
            } else if !settings_changed && !mode_changed {
                backoff_ms = (backoff_ms * 2).min(30_000);
            }

            // Clean up and retry. On a settings or mode change, keep the helper
            // alive (an fps/resolution change is handled at the top of the loop)
            // so the virtual display doesn't flicker off.
            if !settings_changed && !mode_changed {
                if let Some(mut h) = self.helper_child.take() {
                    let _ = h.start_kill();
                }
            }
            if let Some(mut e) = self.encoder_child.take() {
                let _ = e.start_kill();
            }
            encoder_mode = None;
            // Reset codec config so it gets re-extracted on restart
            if let Ok(mut config) = self.codec_config.lock() {
                *config = None;
            }
            if !settings_changed && !mode_changed {
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
        latency: crate::latency::LatencyTracker,
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
                    latency.on_encoded(data.seq);
                    let _ = tx.send(data);
                }
            }
            latency.maybe_report();

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
        if let Some(task) = self.helper_stdout_task.take() {
            task.abort();
        }
        if let Some(mut child) = self.helper_child.take() {
            let _ = child.start_kill();
        }
        if let Some(mut child) = self.encoder_child.take() {
            let _ = child.start_kill();
        }
    }

    /// Stop the pipeline and wait for the children to actually be gone.
    ///
    /// The synchronous [`stop`] only *sends* signals, so the daemon could exit
    /// while ffmpeg was still holding the capture FIFO. That window collides
    /// with an immediate restart — which is exactly what "Apply & restart" in
    /// the GUI does — and two processes on one FIFO produce torn frames.
    pub async fn shutdown(&mut self) {
        if let Some(task) = self.helper_stdout_task.take() {
            task.abort();
        }
        if let Some(mut child) = self.encoder_child.take() {
            Self::terminate(&mut child, "ffmpeg").await;
        }
        if let Some(mut child) = self.helper_child.take() {
            Self::terminate(&mut child, "evdi_helper").await;
        }
        let _ = std::fs::remove_file(FIFO_PATH);
    }

    /// SIGTERM first, then reap. The helper installs a SIGTERM handler and uses
    /// it to run `evdi_disconnect`; SIGKILL skips that and leaves the connector
    /// attached until the kernel gets around to releasing the fd.
    async fn terminate(child: &mut Child, what: &str) {
        let Some(pid) = child.id() else { return };
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        match tokio::time::timeout(std::time::Duration::from_millis(1500), child.wait()).await {
            Ok(Ok(_)) => info!("{} exited cleanly", what),
            _ => {
                warn!("{} ignored SIGTERM — killing", what);
                let _ = child.start_kill();
                let _ =
                    tokio::time::timeout(std::time::Duration::from_millis(500), child.wait()).await;
            }
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
    next_seq: u32,
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
            next_seq: 0,
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
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        Some(VideoPacket {
            data,
            is_idr: was_idr,
            seq,
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
