//! Hyprland: the two jobs KWin's tools do for us on Plasma — switching the
//! virtual output on and pinning the touch and pen devices to it — done
//! through `hyprctl`.
//!
//! Without this Hyprland lists the EVDI output with its modes and leaves it at
//! 0x0, so nothing is ever rendered to it and the tablet stays black (#19).
//!
//! Everything here is a runtime `hyprctl keyword`, never an edit to the
//! user's config: a monitor rule of their own for the output always wins,
//! because the output is only touched when Hyprland left it switched off.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use tracing::{info, warn};

/// The instance signature `hyprctl` needs, for a Hyprland that is running.
///
/// A systemd user service does not always inherit
/// `HYPRLAND_INSTANCE_SIGNATURE` (only when the session exports its
/// environment to systemd), so the newest instance directory under the
/// runtime dir is the fallback — the same place `hyprctl` looks. Either way
/// the socket has to accept a connection: a crashed Hyprland leaves its
/// directory behind, and the user manager can keep an old session's variable
/// into a later Plasma login, and neither must turn a KDE machine into a
/// Hyprland one.
pub fn instance() -> Option<String> {
    let dir = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").ok()?).join("hypr");
    let live = |sig: &str| UnixStream::connect(dir.join(sig).join(".socket.sock")).is_ok();
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        if !sig.is_empty() && live(&sig) {
            return Some(sig);
        }
    }
    let mut found: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((modified, e.file_name().to_string_lossy().into_owned()))
        })
        .collect();
    found.sort();
    found.into_iter().rev().map(|(_, sig)| sig).find(|sig| live(sig))
}

/// Whether this is a Hyprland session we can talk to.
pub fn active() -> bool {
    instance().is_some()
}

async fn hyprctl(args: &[&str]) -> Option<String> {
    let sig = instance()?;
    let out = tokio::process::Command::new("hyprctl")
        .env("HYPRLAND_INSTANCE_SIGNATURE", sig)
        .args(args)
        .output()
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `hyprctl keyword`, logged with Hyprland's answer: it says "ok" or the
/// reason, and exits 0 either way.
async fn keyword(key: &str, value: &str) -> bool {
    match hyprctl(&["keyword", key, value]).await {
        Some(reply) if reply == "ok" => {
            info!("hyprctl keyword {} {}: ok", key, value);
            true
        }
        Some(reply) => {
            warn!("hyprctl keyword {} {}: {}", key, value, reply);
            false
        }
        None => {
            warn!("hyprctl could not be run");
            false
        }
    }
}

#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    pub x: f64,
    pub y: f64,
    /// Current mode in pixels; 0x0 while Hyprland has not switched it on.
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub disabled: bool,
    pub focused: bool,
    /// Largest advertised mode, e.g. 3000x2120 from "3000x2120@60.00Hz".
    pub best_mode: Option<(u32, u32)>,
}

impl Monitor {
    pub fn is_on(&self) -> bool {
        !self.disabled && self.width > 0.0 && self.height > 0.0
    }
    /// The box this monitor covers on the desktop, in layout coordinates.
    pub fn logical(&self) -> (f64, f64, f64, f64) {
        let s = self.scale.max(0.01);
        (self.x, self.y, self.width / s, self.height / s)
    }
}

/// Every monitor Hyprland knows, including the ones it left off.
pub async fn monitors() -> Option<Vec<Monitor>> {
    parse_monitors(&hyprctl(&["-j", "monitors", "all"]).await?)
}

fn parse_monitors(json: &str) -> Option<Vec<Monitor>> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let f = |m: &serde_json::Value, k: &str| m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    Some(
        v.as_array()?
            .iter()
            .map(|m| Monitor {
                name: m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                x: f(m, "x"),
                y: f(m, "y"),
                width: f(m, "width"),
                height: f(m, "height"),
                scale: f(m, "scale"),
                disabled: m.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false),
                focused: m.get("focused").and_then(|v| v.as_bool()).unwrap_or(false),
                best_mode: m
                    .get("availableModes")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|s| {
                        let (w, rest) = s.as_str()?.split_once('x')?;
                        let h = rest.split('@').next()?;
                        Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
                    })
                    .max_by_key(|(w, h)| w * h),
            })
            .collect(),
    )
}

/// An integer scale, so the logical size is whole and Hyprland has nothing
/// to round: 2 for a tablet-class panel (its desktop is still at least 1280
/// wide), 1 otherwise. Fractional scaling is left to a rule of the user's.
fn scale_for(w: u32, h: u32) -> u32 {
    if w.is_multiple_of(2) && h.is_multiple_of(2) && w / 2 >= 1280 {
        2
    } else {
        1
    }
}

/// Switch one of `names` on if Hyprland left it off, placed on the requested
/// side of the other screens. Called on every pass of the capture loop, so it
/// does nothing at all while the output is already on.
pub async fn enable_output(names: &[String], position: crate::config::Position) {
    // The output shows up a moment after the helper connects, and Hyprland
    // may still be applying a rule of its own to it — so give it a second
    // before deciding it was left off.
    let mut seen_off = 0;
    for attempt in 0..15 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let Some(all) = monitors().await else { continue };
        let Some(mon) = all.iter().find(|m| names.contains(&m.name)) else {
            continue;
        };
        if mon.is_on() {
            return;
        }
        seen_off += 1;
        if seen_off < 5 {
            continue;
        }

        let side = match position {
            crate::config::Position::Right => "auto-right",
            crate::config::Position::Left => "auto-left",
            crate::config::Position::Above => "auto-up",
            crate::config::Position::Below => "auto-down",
        };
        let scale = mon.best_mode.map(|(w, h)| scale_for(w, h)).unwrap_or(1);
        info!("Hyprland left {} off — switching it on", mon.name);
        keyword("monitor", &format!("{},preferred,{},{}", mon.name, side, scale)).await;

        for _ in 0..10 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let on = monitors()
                .await
                .and_then(|all| all.into_iter().find(|m| m.name == mon.name))
                .is_some_and(|m| m.is_on());
            if on {
                return;
            }
        }
        let (w, h) = mon.best_mode.unwrap_or((1920, 1200));
        warn!(
            "Hyprland did not switch {} on. By hand, at half resolution: \
             hyprctl keyword monitor {},{}x{}@60,{},1",
            mon.name, mon.name, w / 2, h / 2, side
        );
        return;
    }
    warn!("The EVDI output did not appear in `hyprctl monitors` within 3s");
}

/// Switch the output off so Hyprland moves its workspaces back to the real
/// screens. The rule replaces ours, and the next `enable_output` replaces it.
pub async fn disable_output(names: &[String]) {
    let Some(all) = monitors().await else { return };
    for mon in all.iter().filter(|m| names.contains(&m.name) && m.is_on()) {
        info!("Disabling Hyprland output {}", mon.name);
        keyword("monitor", &format!("{},disable", mon.name)).await;
    }
}

/// The first of `names` that is switched on, waiting up to `timeout`.
pub async fn wait_on(names: &[String], timeout: std::time::Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(m) = monitors()
            .await
            .and_then(|all| all.into_iter().find(|m| names.contains(&m.name) && m.is_on()))
        {
            return Some(m.name);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// The screen the user is looking at, for graphics-tablet mode: the focused
/// monitor unless it is one of ours, else the first real one that is on.
pub async fn primary_output(ours: &[String]) -> Option<String> {
    let all = monitors().await?;
    let real: Vec<&Monitor> = all.iter().filter(|m| m.is_on() && !ours.contains(&m.name)).collect();
    real.iter()
        .find(|m| m.focused)
        .or_else(|| real.first())
        .map(|m| m.name.clone())
}

/// Hyprland's name for an input device: lower case, spaces as dashes, which is
/// what `hyprctl devices` prints and what `device[...]` has to match.
fn device_key(name: &str) -> String {
    name.replace([' ', '\n'], "-").to_lowercase()
}

/// Pin each named device to `output`. Returns how many Hyprland accepted.
pub async fn map_devices(device_names: &[&str], output: &str) -> usize {
    // Only devices Hyprland has registered: a keyword for a name it does not
    // know yet is accepted and then applies to nothing.
    let listed = hyprctl(&["-j", "devices"]).await.unwrap_or_default();
    let mut mapped = 0;
    for name in device_names {
        let key = device_key(name);
        if !listed.contains(&format!("\"{}\"", key)) {
            continue;
        }
        // `device[name]:option` is the current spelling; `device:name:option`
        // is what Hyprland took before per-device blocks got a `name` field.
        if keyword(&format!("device[{}]:output", key), output).await
            || keyword(&format!("device:{}:output", key), output).await
        {
            info!("Mapped '{}' to output {}", name, output);
            mapped += 1;
        }
    }
    mapped
}

/// Where `output` sits on the desktop, as fractions of the whole layout's
/// bounding box: (x, y, width, height). Same contract as the KDE version.
pub async fn output_area(output: &str) -> Option<(f64, f64, f64, f64)> {
    let all = monitors().await?;
    let on: Vec<(f64, f64, f64, f64)> = all.iter().filter(|m| m.is_on()).map(Monitor::logical).collect();
    let (x, y, w, h) = all.iter().find(|m| m.name == output && m.is_on())?.logical();
    let min_x = on.iter().map(|b| b.0).fold(f64::INFINITY, f64::min);
    let min_y = on.iter().map(|b| b.1).fold(f64::INFINITY, f64::min);
    let max_x = on.iter().map(|b| b.0 + b.2).fold(f64::NEG_INFINITY, f64::max);
    let max_y = on.iter().map(|b| b.1 + b.3).fold(f64::NEG_INFINITY, f64::max);
    let (tw, th) = (max_x - min_x, max_y - min_y);
    (tw > 0.0 && th > 0.0).then(|| ((x - min_x) / tw, (y - min_y) / th, w / tw, h / th))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_keys_match_hyprctl_devices() {
        assert_eq!(device_key("UScreen Pen"), "uscreen-pen");
        assert_eq!(device_key("UScreen Touch 2"), "uscreen-touch-2");
    }

    #[test]
    fn reads_an_output_hyprland_left_off() {
        // Shaped like #19: the laptop panel on, the EVDI output listed with
        // its mode but at 0x0.
        let json = r#"[
          {"id":0,"name":"eDP-1","width":1920,"height":1080,"refreshRate":60.0,
           "x":0,"y":0,"scale":1.6,"focused":true,"disabled":false,
           "availableModes":["1920x1080@60.00Hz"]},
          {"id":1,"name":"DVI-I-2","width":0,"height":0,"refreshRate":60.0,
           "x":0,"y":0,"scale":1.0,"focused":false,"disabled":true,
           "availableModes":["3000x2120@60.00Hz"]}
        ]"#;
        let m = parse_monitors(json).unwrap();
        assert!(m[0].is_on());
        assert!(!m[1].is_on());
        assert_eq!(m[1].best_mode, Some((3000, 2120)));
        assert_eq!(m[0].logical(), (0.0, 0.0, 1200.0, 675.0));
    }

    #[test]
    fn scale_is_whole_and_tablet_sized() {
        assert_eq!(scale_for(3000, 2120), 2);
        assert_eq!(scale_for(2960, 1848), 2);
        assert_eq!(scale_for(1920, 1200), 1);
        assert_eq!(scale_for(2561, 1600), 1);
    }
}
