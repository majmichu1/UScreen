use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Highest bitrate the USB transport actually sustains. Beyond this the encoder
/// outruns the link, frames pile up in every queue along the way and latency
/// grows without bound — the stream does not get sharper, only later.
///
/// This is a hard ceiling rather than a hint because the value is persisted:
/// a bad number pushed once from the tablet stays in the config file and keeps
/// poisoning every subsequent run.
pub const MAX_BITRATE_KBPS: u32 = 60_000;
pub const MIN_BITRATE_KBPS: u32 = 1_000;

/// The generated EDID caps the virtual mode at 90 Hz (EDID 1.4 stores the pixel
/// clock in 16 bits, and 2960x1848@120 overflows it), so anything above 90
/// would only ever produce duplicate frames.
pub const MAX_FPS: u32 = 90;
pub const MIN_FPS: u32 = 10;

/// Constant-quality target for the encoder (lower = sharper, more bits).
///
/// 18 rather than a more conservative value because bandwidth stopped being the
/// constraint: in constant-quality mode a desktop streams at a few Mbps against
/// a ceiling tens of times higher, so spending bits on crisp text is close to
/// free. Text sharpness is ultimately limited by 4:2:0 chroma subsampling, not
/// by this number — below roughly 16 there is nothing left to gain.
pub const DEFAULT_QUALITY: u32 = 18;
pub const MIN_QUALITY: u32 = 12;
pub const MAX_QUALITY: u32 = 32;

/// Persistent settings, shared by the CLI daemon, the GUI and the tablet app
/// (which pushes changes over the input WebSocket).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FileConfig {
    pub encoder: String,
    pub fps: u32,
    /// kbps
    pub bitrate: u32,
    pub width: u32,
    pub height: u32,
    /// Encoder constant-quality target: lower is sharper and costs more bits.
    ///
    /// This, not the bitrate, is what governs picture quality now that the
    /// encoder runs in constant-quality mode — the bitrate is only a ceiling
    /// for bursts, and on a desktop the stream sits far below it.
    pub quality: u32,
    /// Integer downscale for the stream only; the desktop keeps its native
    /// mode. 1 = native, 2 = half. Trades sharpness for latency: the tablet's
    /// decoder costs roughly 7-8ms fixed plus 1.2ms per megapixel, so fewer
    /// pixels arrive on screen sooner.
    pub stream_scale: u32,
    /// Use the tablet as a graphics tablet for the laptop's own screen rather
    /// than as a second display: no capture, no encoding, nothing streamed —
    /// the pen and touch simply drive the screen you are already looking at.
    /// For drawing that removes display latency from the loop entirely, which
    /// is worth more than any amount of tuning the video path.
    pub pen_only: bool,
    /// Where the virtual screen sits relative to the physical ones:
    /// "right" (default), "left", "above" or "below". Anything else is
    /// treated as "right" rather than refusing to start over a typo.
    pub position: String,
    /// Encode in 10 bits (HEVC Main10). The captured desktop is 8-bit and
    /// cannot be otherwise, so this adds no colour — it gives the encoder
    /// more precision to work in, which is what removes banding from
    /// gradients at a given bitrate. Costs a format conversion per frame.
    pub ten_bit: bool,
    /// Require the tablet to present this run's session token before it is
    /// sent any video or allowed to inject input. The token is handed to the
    /// app over adb when it is launched. Without this, any local process —
    /// or any other app on the tablet — could connect to the loopback ports,
    /// read the screen and drive the mouse. Off only if you need an app
    /// older than 1.1.0 to keep working.
    pub require_token: bool,
    /// Ask GitHub once a day whether a newer release exists, and say so in
    /// the tray and in `uscreen doctor`. Nothing is ever downloaded or
    /// installed by the daemon; updating stays with you or your package
    /// manager. One HTTPS request a day to api.github.com.
    pub check_updates: bool,
    /// Match the virtual display to whatever resolution the tablet reports
    pub auto_resolution: bool,
    pub video_port: u16,
    pub input_port: u16,
    /// Launch the UScreen app on the tablet automatically when it's plugged in
    pub auto_launch_app: bool,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            encoder: "h264_nvenc".into(),
            fps: 60,
            bitrate: 20000,
            width: 2960,
            height: 1848,
            quality: DEFAULT_QUALITY,
            stream_scale: 1,
            pen_only: false,
            position: "right".into(),
            ten_bit: false,
            require_token: true,
            check_updates: true,
            auto_resolution: true,
            video_port: 8890,
            input_port: 8891,
            auto_launch_app: true,
        }
    }
}

/// Where the virtual screen goes relative to everything already on the desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    Right,
    Left,
    Above,
    Below,
}

impl Position {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "right" => Some(Position::Right),
            "left" => Some(Position::Left),
            "above" | "top" => Some(Position::Above),
            "below" | "bottom" => Some(Position::Below),
            _ => None,
        }
    }

    /// Unknown values are a typo in a config file, not a reason to refuse to
    /// bring the screen up at all.
    pub fn parse_or_default(s: &str) -> Self {
        Self::parse(s).unwrap_or(Position::Right)
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/uscreen/config.toml")
}

impl FileConfig {
    pub fn load() -> Self {
        let path = config_path();
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("Invalid config at {:?}: {} — using defaults", path, e);
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        cfg.sanitize();
        cfg
    }

    /// Pull persisted values back into the range the pipeline can actually
    /// serve. Older builds let the tablet push 200 Mbps @ 90 fps and wrote it
    /// straight to disk, so existing installs carry settings that guarantee
    /// multi-second latency until they are clamped here.
    pub fn sanitize(&mut self) {
        let bitrate = self.bitrate.clamp(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS);
        if bitrate != self.bitrate {
            tracing::warn!(
                "Bitrate {} kbps is beyond what the USB transport sustains — clamped to {} kbps",
                self.bitrate,
                bitrate
            );
            self.bitrate = bitrate;
        }

        let fps = self.fps.clamp(MIN_FPS, MAX_FPS);
        if fps != self.fps {
            tracing::warn!("fps {} out of range — clamped to {}", self.fps, fps);
            self.fps = fps;
        }

        self.quality = self.quality.clamp(MIN_QUALITY, MAX_QUALITY);
        self.stream_scale = self.stream_scale.clamp(1, 4);
        self.width = self.width.clamp(640, 8192);
        self.height = self.height.clamp(480, 8192);

        if Position::parse(&self.position).is_none() {
            tracing::warn!(
                "Unknown position {:?} — falling back to \"right\"",
                self.position
            );
            self.position = "right".into();
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(&path, text).context("write config file")?;
        Ok(())
    }
}
