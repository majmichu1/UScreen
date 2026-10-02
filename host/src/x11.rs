//! X11 sessions that are not Plasma — Cinnamon, XFCE, MATE, i3, GNOME on
//! Xorg: switch the virtual output on with `xrandr` and pin the tablet's
//! devices to it with `xinput map-to-output`.
//!
//! Plasma has its own path (KWin over D-Bus) and is left to it, also on X11.
//! Under Wayland `xrandr` only shows an emulation, so this is for real X
//! sessions only.
//!
//! The pointer device is mapped too, unlike on KWin: `map-to-output` works on
//! any absolute device, so the cursor parked where the pen was lifted needs
//! no desktop-wide conversion here.
//!
//! Written from the `xrandr` and `xinput` manuals without an X11 desktop to
//! run it on; every command's outcome is logged.

use tracing::{info, warn};

fn command_exists(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| std::path::Path::new(d).join(name).exists())
}

pub fn active() -> bool {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
    session == "x11"
        && !desktop.contains("KDE")
        && std::env::var_os("DISPLAY").is_some()
        && command_exists("xrandr")
        && command_exists("xinput")
}

async fn run(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = tokio::process::Command::new(program).args(args).output().await.ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some((out.status.success(), text))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub connected: bool,
    /// Width, height, x, y — present only while the output shows something.
    pub geometry: Option<(i64, i64, i64, i64)>,
    pub primary: bool,
}

/// `1920x1080+0+0` → (1920, 1080, 0, 0).
fn parse_geometry(token: &str) -> Option<(i64, i64, i64, i64)> {
    let (size, rest) = token.split_once('+')?;
    let (w, h) = size.split_once('x')?;
    let (x, y) = rest.split_once('+')?;
    Some((w.parse().ok()?, h.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
}

fn parse_outputs(text: &str) -> Vec<Output> {
    let mut outs = Vec::new();
    for line in text.lines() {
        // Mode lines are indented; output lines start at column 0.
        if line.starts_with(' ') || line.starts_with("Screen") {
            continue;
        }
        let mut t = line.split_whitespace();
        let (Some(name), Some(state)) = (t.next(), t.next()) else { continue };
        if state != "connected" && state != "disconnected" {
            continue;
        }
        let rest: Vec<&str> = t.collect();
        outs.push(Output {
            name: name.to_string(),
            connected: state == "connected",
            geometry: rest.iter().take(2).find_map(|x| parse_geometry(x)),
            primary: rest.first() == Some(&"primary"),
        });
    }
    outs
}

async fn outputs() -> Option<Vec<Output>> {
    let (ok, text) = run("xrandr", &["--query"]).await?;
    ok.then(|| parse_outputs(&text))
}

/// Switch one of `names` on if it is connected but showing nothing, on the
/// requested side of the other screens.
pub async fn enable_output(names: &[String], position: crate::config::Position) {
    for attempt in 0..15 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let Some(all) = outputs().await else { continue };
        let Some(mine) = all.iter().find(|o| o.connected && names.contains(&o.name)) else {
            continue;
        };
        if mine.geometry.is_some() {
            return;
        }
        let active: Vec<&Output> = all
            .iter()
            .filter(|o| o.geometry.is_some() && !names.contains(&o.name))
            .collect();
        // Relative to the screen at the edge it is going next to.
        let edge = |key: fn(&(i64, i64, i64, i64)) -> i64, largest: bool| {
            let mut v: Vec<&&Output> = active.iter().collect();
            v.sort_by_key(|o| key(&o.geometry.unwrap()));
            if largest { v.last().copied() } else { v.first().copied() }.map(|o| o.name.clone())
        };
        use crate::config::Position::*;
        let (flag, reference) = match position {
            Right => ("--right-of", edge(|g| g.2 + g.0, true)),
            Left => ("--left-of", edge(|g| g.2, false)),
            Above => ("--above", edge(|g| g.3, false)),
            Below => ("--below", edge(|g| g.3 + g.1, true)),
        };
        let mut args = vec!["--output", mine.name.as_str(), "--auto"];
        if let Some(r) = reference.as_deref() {
            args.extend([flag, r]);
        }
        info!("X11 left {} off — xrandr {}", mine.name, args.join(" "));
        match run("xrandr", &args).await {
            Some((true, _)) => info!("xrandr enable {}: ok", mine.name),
            Some((false, text)) => warn!("xrandr failed: {}", text.trim()),
            None => warn!("xrandr could not be run"),
        }
        return;
    }
    warn!("The EVDI output did not appear in `xrandr --query` within 3s");
}

pub async fn disable_output(names: &[String]) {
    let Some(all) = outputs().await else { return };
    for o in all.iter().filter(|o| names.contains(&o.name) && o.geometry.is_some()) {
        info!("Disabling X11 output {}", o.name);
        let _ = run("xrandr", &["--output", &o.name, "--off"]).await;
    }
}

/// The first of `names` that is showing something, waiting up to `timeout`.
pub async fn wait_active(names: &[String], timeout: std::time::Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(o) = outputs()
            .await
            .and_then(|all| all.into_iter().find(|o| o.geometry.is_some() && names.contains(&o.name)))
        {
            return Some(o.name);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// The screen the user is looking at, for graphics-tablet mode: the primary
/// one unless it is ours, else the first real one that is on.
pub async fn primary_output(ours: &[String]) -> Option<String> {
    let all = outputs().await?;
    let real: Vec<&Output> = all.iter().filter(|o| o.geometry.is_some() && !ours.contains(&o.name)).collect();
    real.iter().find(|o| o.primary).or_else(|| real.first()).map(|o| o.name.clone())
}

/// Whether the `xinput` device `listed` is `wanted` or one of its sub-devices
/// (`"UScreen Pen Pen (0)"`), and not the next tablet's `"UScreen Pen 2"`.
fn belongs_to(listed: &str, wanted: &str) -> bool {
    match listed.strip_prefix(wanted) {
        Some("") => true,
        Some(rest) => {
            rest.starts_with(' ') && !rest.trim().chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// Pin each of `wanted` to `output`. Returns how many of them `xinput` knew
/// and accepted.
pub async fn map_devices(wanted: &[&str], output: &str) -> usize {
    let Some((true, listing)) = run("xinput", &["list", "--name-only"]).await else {
        return 0;
    };
    let mut done = 0;
    for w in wanted {
        let mut ok = false;
        for listed in listing.lines().map(str::trim).filter(|l| belongs_to(l, w)) {
            match run("xinput", &["map-to-output", listed, output]).await {
                Some((true, _)) => {
                    info!("Mapped '{}' to output {}", listed, output);
                    ok = true;
                }
                Some((false, text)) => warn!("xinput map-to-output '{}': {}", listed, text.trim()),
                None => warn!("xinput could not be run"),
            }
        }
        done += ok as usize;
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY: &str = "Screen 0: minimum 320 x 200, current 1920 x 1080, maximum 16384 x 16384\n\
eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm\n\
   1920x1080     60.01*+  48.01\n\
HDMI-1 disconnected (normal left inverted right x axis y axis)\n\
DVI-I-1 connected (normal left inverted right x axis y axis) 314mm x 195mm\n\
   2960x1848     90.00 +\n\
DP-2 connected 1280x1024+1920+0 (normal left inverted right x axis y axis) 376mm x 301mm\n";

    #[test]
    fn reads_which_outputs_are_on() {
        let o = parse_outputs(QUERY);
        assert_eq!(o.len(), 4);
        assert_eq!(o[0].geometry, Some((1920, 1080, 0, 0)));
        assert!(o[0].primary);
        assert!(!o[1].connected);
        assert_eq!(o[2].name, "DVI-I-1");
        assert!(o[2].connected && o[2].geometry.is_none());
        assert_eq!(o[3].geometry, Some((1280, 1024, 1920, 0)));
        assert!(!o[3].primary);
    }

    #[test]
    fn matches_a_device_and_its_sub_devices_but_not_the_next_tablet() {
        assert!(belongs_to("UScreen Pen", "UScreen Pen"));
        assert!(belongs_to("UScreen Pen Pen (0)", "UScreen Pen"));
        assert!(!belongs_to("UScreen Pen 2", "UScreen Pen"));
        assert!(!belongs_to("UScreen Pointer", "UScreen Pen"));
        assert!(belongs_to("UScreen Pen 2", "UScreen Pen 2"));
    }
}
