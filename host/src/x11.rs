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
    /// The output's EDID as `xrandr --prop` prints it; empty if it has none.
    pub edid: Vec<u8>,
}

/// One of our virtual outputs, as the kernel knows it.
struct Target {
    name: String,
    edid: Vec<u8>,
}

fn targets_for(names: &[String]) -> Vec<Target> {
    crate::vdisplay::evdi_connectors()
        .into_iter()
        .filter(|c| names.contains(&c.name))
        .map(|c| Target { name: c.name, edid: c.edid })
        .collect()
}

/// Whether the X output `o` is one of `targets`.
///
/// Xorg does not call an EVDI output by its kernel name: an output of a
/// secondary GPU gets a suffix, so `DVI-I-1` in sysfs shows up as `DVI-I-1-1`
/// in xrandr. The EDID is the reliable identity — it is the one we wrote — and
/// it also keeps a real DVI monitor that really is called `DVI-I-1` from being
/// taken for ours. Only when an output has no EDID to compare does the name
/// decide, with the suffix allowed.
fn is_ours(o: &Output, targets: &[Target]) -> bool {
    targets.iter().any(|t| {
        if !o.edid.is_empty() && !t.edid.is_empty() {
            return o.edid == t.edid;
        }
        o.name == t.name
            || o.name
                .strip_prefix(t.name.as_str())
                .and_then(|r| r.strip_prefix('-'))
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
    })
}

/// `1920x1080+0+0` → (1920, 1080, 0, 0).
fn parse_geometry(token: &str) -> Option<(i64, i64, i64, i64)> {
    let (size, rest) = token.split_once('+')?;
    let (w, h) = size.split_once('x')?;
    let (x, y) = rest.split_once('+')?;
    Some((w.parse().ok()?, h.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn parse_outputs(text: &str) -> Vec<Output> {
    let mut outs: Vec<Output> = Vec::new();
    let mut in_edid = false;
    for line in text.lines() {
        // Mode and property lines are indented; output lines start at column 0.
        if line.starts_with(char::is_whitespace) {
            let t = line.trim();
            if t == "EDID:" {
                in_edid = true;
            } else if in_edid {
                match (hex_bytes(t), outs.last_mut()) {
                    (Some(b), Some(o)) => o.edid.extend(b),
                    _ => in_edid = false,
                }
            }
            continue;
        }
        in_edid = false;
        if line.starts_with("Screen") {
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
            edid: Vec::new(),
        });
    }
    outs
}

async fn outputs() -> Option<Vec<Output>> {
    let (ok, text) = run("xrandr", &["--prop"]).await?;
    ok.then(|| parse_outputs(&text))
}

/// Switch one of `names` on if it is connected but showing nothing, on the
/// requested side of the other screens.
pub async fn enable_output(names: &[String], position: crate::config::Position) {
    let targets = targets_for(names);
    for attempt in 0..15 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let Some(all) = outputs().await else { continue };
        let Some(mine) = all.iter().find(|o| o.connected && is_ours(o, &targets)) else {
            continue;
        };
        if mine.geometry.is_some() {
            return;
        }
        let active: Vec<&Output> = all
            .iter()
            .filter(|o| o.geometry.is_some() && !is_ours(o, &targets))
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
    warn!("The EVDI output did not appear in `xrandr --prop` within 3s");
}

pub async fn disable_output(names: &[String]) {
    let targets = targets_for(names);
    let Some(all) = outputs().await else { return };
    for o in all.iter().filter(|o| is_ours(o, &targets) && o.geometry.is_some()) {
        info!("Disabling X11 output {}", o.name);
        let _ = run("xrandr", &["--output", &o.name, "--off"]).await;
    }
}

/// The first of `names` that is showing something, waiting up to `timeout`.
pub async fn wait_active(names: &[String], timeout: std::time::Duration) -> Option<String> {
    let targets = targets_for(names);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(o) = outputs()
            .await
            .and_then(|all| all.into_iter().find(|o| o.geometry.is_some() && is_ours(o, &targets)))
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
    let targets = targets_for(ours);
    let all = outputs().await?;
    let real: Vec<&Output> = all.iter().filter(|o| o.geometry.is_some() && !is_ours(o, &targets)).collect();
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

    fn hex_block(edid: &[u8]) -> String {
        edid.chunks(16)
            .map(|c| format!("\t\t{}\n", c.iter().map(|b| format!("{:02x}", b)).collect::<String>()))
            .collect()
    }

    fn prop_output(edid_ours: &[u8], edid_real: &[u8]) -> String {
        format!(
            "Screen 0: minimum 320 x 200, current 1920 x 1080, maximum 16384 x 16384\n\
eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm\n\
\tEDID:\n{real}\tscaling mode: Full aspect\n\
   1920x1080     60.01*+  48.01\n\
HDMI-1 disconnected (normal left inverted right x axis y axis)\n\
DVI-I-1 connected 1280x1024+1920+0 (normal left inverted right x axis y axis) 376mm x 301mm\n\
\tEDID:\n{real}\
DVI-I-1-1 connected (normal left inverted right x axis y axis) 314mm x 195mm\n\
\tEDID:\n{ours}\tBorder: 0 0 0 0\n\
   2960x1848     90.00 +\n",
            real = hex_block(edid_real),
            ours = hex_block(edid_ours),
        )
    }

    #[test]
    fn reads_outputs_and_their_edid_from_xrandr_prop() {
        let ours = crate::edid::make_edid(2960, 1848, 90);
        let real = vec![0x11u8; 128];
        let o = parse_outputs(&prop_output(&ours, &real));
        assert_eq!(o.len(), 4);
        assert_eq!(o[0].geometry, Some((1920, 1080, 0, 0)));
        assert!(o[0].primary && o[0].edid == real);
        assert!(!o[1].connected);
        assert_eq!(o[3].name, "DVI-I-1-1");
        assert!(o[3].connected && o[3].geometry.is_none());
        assert_eq!(o[3].edid, ours);
    }

    #[test]
    fn finds_our_output_by_edid_not_by_name() {
        let ours = crate::edid::make_edid(2960, 1848, 90);
        let o = parse_outputs(&prop_output(&ours, &[0x11u8; 128]));
        // The kernel calls it DVI-I-1; so does a real monitor on the main GPU.
        let targets = vec![Target { name: "DVI-I-1".into(), edid: ours }];
        let found: Vec<&str> = o.iter().filter(|x| is_ours(x, &targets)).map(|x| x.name.as_str()).collect();
        assert_eq!(found, ["DVI-I-1-1"]);
    }

    #[test]
    fn falls_back_to_the_suffixed_name_without_an_edid() {
        let o = |name: &str| Output { name: name.into(), connected: true, geometry: None, primary: false, edid: Vec::new() };
        let t = vec![Target { name: "DVI-I-2".into(), edid: Vec::new() }];
        assert!(is_ours(&o("DVI-I-2"), &t));
        assert!(is_ours(&o("DVI-I-2-1"), &t));
        assert!(!is_ours(&o("DVI-I-2-x"), &t));
        assert!(!is_ours(&o("DVI-I-1-1"), &t));
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
