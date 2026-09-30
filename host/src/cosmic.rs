//! COSMIC: switching the virtual output on with `cosmic-randr`.
//!
//! COSMIC leaves a new EVDI output disabled, so nothing is rendered to it, the
//! helper sees almost no frames and the tablet keeps restarting its
//! connection (#26). Only the on/off part is done here; COSMIC has no tool
//! for pinning input devices to an output, so pen and touch mapping stays
//! manual there.
//!
//! Written from `cosmic-randr`'s documented `list` / `enable` / `disable`
//! commands without a COSMIC machine to try them on, so every call logs what
//! the tool answered.

use tracing::{info, warn};

/// A COSMIC session with the tool installed.
pub fn active() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    desktop.to_uppercase().contains("COSMIC") && tool_exists()
}

fn tool_exists() -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| std::path::Path::new(d).join("cosmic-randr").exists())
}

async fn randr(args: &[&str]) -> Option<(bool, String)> {
    let out = tokio::process::Command::new("cosmic-randr")
        .args(args)
        .output()
        .await
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some((out.status.success(), text))
}

/// Drop terminal colour codes, which `cosmic-randr list` may print.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What `cosmic-randr list` says about `name`: `Some(true)` enabled,
/// `Some(false)` disabled, `None` when the output or the wording is not
/// recognised — in which case the caller just enables it.
fn state_in(listing: &str, name: &str) -> Option<bool> {
    let text = plain(listing);
    let line = text.lines().find(|l| l.trim_start().starts_with(name))?;
    let lower = line.to_lowercase();
    if lower.contains("disabled") {
        Some(false)
    } else if lower.contains("enabled") {
        Some(true)
    } else {
        None
    }
}

/// Switch the first of `names` that COSMIC knows on, unless it already is.
pub async fn enable_output(names: &[String]) {
    // The output appears a moment after the helper connects.
    for attempt in 0..15 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let Some((_, listing)) = randr(&["list"]).await else {
            warn!("cosmic-randr could not be run");
            return;
        };
        let Some(name) = names.iter().find(|n| listing.contains(n.as_str())) else {
            continue;
        };
        if state_in(&listing, name) == Some(true) {
            return;
        }
        info!("COSMIC left {} off — switching it on", name);
        match randr(&["enable", name]).await {
            Some((true, _)) => info!("cosmic-randr enable {}: ok", name),
            Some((false, text)) => warn!("cosmic-randr enable {} failed: {}", name, text.trim()),
            None => warn!("cosmic-randr could not be run"),
        }
        return;
    }
    warn!("The EVDI output did not appear in `cosmic-randr list` within 3s");
}

/// Switch the output off so COSMIC moves its windows back to the real screens.
pub async fn disable_output(names: &[String]) {
    let Some((_, listing)) = randr(&["list"]).await else { return };
    for name in names.iter().filter(|n| state_in(&listing, n) == Some(true)) {
        info!("Disabling COSMIC output {}", name);
        let _ = randr(&["disable", name]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_output_state_with_and_without_colour_codes() {
        let listing = "eDP-1 (enabled)\n  Modes:\n    1920x1080 @ 60.000 Hz\n\
                       \x1b[1mDVI-I-2\x1b[0m (disabled)\n  Modes:\n    2112x1320 @ 60.000 Hz\n";
        assert_eq!(state_in(listing, "eDP-1"), Some(true));
        assert_eq!(state_in(listing, "DVI-I-2"), Some(false));
        assert_eq!(state_in(listing, "HDMI-A-1"), None);
        assert_eq!(state_in("DVI-I-2\n", "DVI-I-2"), None);
    }
}
