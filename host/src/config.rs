use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
            auto_resolution: true,
            video_port: 8890,
            input_port: 8891,
            auto_launch_app: true,
        }
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/uscreen/config.toml")
}

impl FileConfig {
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("Invalid config at {:?}: {} — using defaults", path, e);
                Self::default()
            }),
            Err(_) => Self::default(),
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
