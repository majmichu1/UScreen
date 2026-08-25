//! Suppressing the desktop's on-screen keyboard while uscreen is running.
//!
//! The tablet's touch device is a genuine touchscreen as far as the desktop is
//! concerned, so KDE pops the virtual keyboard up whenever a text field is
//! focused by touch or pen. On a screen you are using as a monitor — or that
//! you are only drawing on — that is never what you want.
//!
//! The setting is global to the desktop, so it is saved before being changed
//! and put back on shutdown: a display server should not leave the user's
//! session altered after it exits. The saved value lives in a file rather than
//! in memory so that a daemon which was killed rather than stopped can still be
//! undone by the next run.

use std::path::PathBuf;
use tracing::{info, warn};

const GROUP: &str = "Wayland";
const KEY_ENABLED: &str = "VirtualKeyboardEnabled";
const KEY_MODE: &str = "VirtualKeyboardMode";

fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/uscreen/osk-restore")
}

async fn read_key(key: &str) -> Option<String> {
    let out = tokio::process::Command::new("kreadconfig6")
        .args(["--file", "kwinrc", "--group", GROUP, "--key", key])
        .output()
        .await
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

async fn write_key(key: &str, value: &str) -> bool {
    tokio::process::Command::new("kwriteconfig6")
        .args(["--file", "kwinrc", "--group", GROUP, "--key", key, value])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn reconfigure_kwin() {
    let _ = tokio::process::Command::new("qdbus")
        .args(["org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"])
        .output()
        .await;
}

/// Turn the on-screen keyboard off, remembering what it was set to.
pub async fn disable() {
    let path = state_path();
    // A state file already present means a previous run did not get to restore.
    // Keep that older value: it is the one that reflects what the user chose,
    // whereas the current setting is whatever we left behind.
    if !path.exists() {
        let enabled = read_key(KEY_ENABLED).await.unwrap_or_else(|| "true".into());
        let mode = read_key(KEY_MODE).await.unwrap_or_else(|| "1".into());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, format!("{}\n{}\n", enabled, mode)) {
            warn!("Could not save the on-screen keyboard setting: {}", e);
            return; // Without a way back, leave the user's desktop alone.
        }
    }

    if write_key(KEY_ENABLED, "false").await {
        reconfigure_kwin().await;
        info!("On-screen keyboard suppressed while uscreen runs");
    }
}

/// Put the on-screen keyboard back the way the user had it.
pub async fn restore() {
    let path = state_path();
    let Ok(saved) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut lines = saved.lines();
    let enabled = lines.next().unwrap_or("true");
    let mode = lines.next().unwrap_or("1");
    write_key(KEY_ENABLED, enabled).await;
    write_key(KEY_MODE, mode).await;
    reconfigure_kwin().await;
    let _ = std::fs::remove_file(&path);
    info!("On-screen keyboard setting restored");
}
