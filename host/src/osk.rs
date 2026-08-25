//! Suppressing the desktop's on-screen keyboard while uscreen is running.
//!
//! The tablet's touch device is a genuine touchscreen as far as the desktop is
//! concerned, so KDE offers the virtual keyboard whenever a text field takes
//! focus. On a screen being used as a monitor — or one you are only drawing on
//! — that is never wanted.
//!
//! Done over KWin's D-Bus interface rather than by writing kwinrc. Writing the
//! config file looks like the obvious route and does change the value on disk,
//! but KWin does not re-read it: verified by setting `VirtualKeyboardMode=0` in
//! the file and finding the live property still reporting 1, with the keyboard
//! duly appearing. Setting the property takes effect immediately, and has the
//! further advantage of leaving the user's saved configuration untouched.
//!
//! The previous value still goes to disk, so a daemon that was killed rather
//! than stopped can be undone by the next run.

use std::path::PathBuf;
use tracing::{info, warn};

const OBJECT: &str = "/VirtualKeyboard";
const IFACE: &str = "org.kde.kwin.VirtualKeyboard";
/// Only when explicitly asked for, rather than on touch input.
const MODE_MANUAL: &str = "0";

fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/uscreen/osk-restore")
}

async fn get_mode() -> Option<String> {
    let out = tokio::process::Command::new("qdbus")
        .args([
            "--literal",
            "org.kde.KWin",
            OBJECT,
            "org.freedesktop.DBus.Properties.Get",
            IFACE,
            "mode",
        ])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // qdbus --literal prints e.g. [Variant(int): 1]
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    (!digits.is_empty()).then_some(digits)
}

async fn set_mode(mode: &str) -> bool {
    tokio::process::Command::new("qdbus")
        .args([
            "--literal",
            "org.kde.KWin",
            OBJECT,
            "org.freedesktop.DBus.Properties.Set",
            IFACE,
            "mode",
            mode,
        ])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Turn the on-screen keyboard off, remembering how it was set.
pub async fn disable() {
    let path = state_path();
    // A state file already present means a previous run never restored. Keep
    // that value: it is the user's, whereas the current one is ours.
    if !path.exists() {
        let Some(current) = get_mode().await else {
            warn!("KWin's virtual keyboard interface is unavailable — leaving it alone");
            return;
        };
        if current == MODE_MANUAL {
            return; // Already how we want it; nothing to remember or undo.
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, &current).is_err() {
            warn!("Could not save the keyboard setting — leaving it alone");
            return; // Without a way back, do not touch the user's desktop.
        }
    }

    if set_mode(MODE_MANUAL).await {
        info!("On-screen keyboard suppressed while uscreen runs");
    }
}

/// Put the on-screen keyboard back the way the user had it.
pub async fn restore() {
    let path = state_path();
    let Ok(saved) = std::fs::read_to_string(&path) else {
        return;
    };
    let saved = saved.trim();
    if !saved.is_empty() && set_mode(saved).await {
        info!("On-screen keyboard setting restored");
    }
    let _ = std::fs::remove_file(&path);
}
