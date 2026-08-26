//! `uscreen doctor` — one-shot check of everything that has to be true for the
//! pipeline to work, with an actionable hint for every failure.
//!
//! This is deliberately the first thing to run whenever the stream "is laggy
//! again". The most common cause is not the pipeline at all but orphaned
//! `evdi_helper`/`ffmpeg` processes from a previous run: several writers on the
//! shared capture FIFO interleave at pipe granularity, and no amount of
//! restarting the daemon fixes it until they are killed.

use crate::capture::FIFO_PATH;
use crate::config::{self, FileConfig, MAX_BITRATE_KBPS, MAX_FPS};
use crate::vdisplay;
use anyhow::Result;
use std::path::Path;

#[derive(PartialEq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Report {
    warnings: u32,
    failures: u32,
}

impl Report {
    fn new() -> Self {
        Self {
            warnings: 0,
            failures: 0,
        }
    }

    fn line(&mut self, level: Level, label: &str, detail: &str) {
        let mark = match level {
            Level::Ok => "  ok  ",
            Level::Warn => " warn ",
            Level::Fail => " FAIL ",
        };
        match level {
            Level::Warn => self.warnings += 1,
            Level::Fail => self.failures += 1,
            Level::Ok => {}
        }
        if detail.is_empty() {
            println!("[{}] {}", mark, label);
        } else {
            println!("[{}] {:<34} {}", mark, label, detail);
        }
    }

    /// A remedy printed under the finding it belongs to.
    fn hint(&self, text: &str) {
        println!("         → {}", text);
    }
}

fn section(title: &str) {
    println!("\n{}", title);
    println!("{}", "-".repeat(title.len()));
}

async fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn command_exists(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|dir| Path::new(dir).join(name).exists())
}

/// PIDs whose executable name matches exactly (`pgrep -x`).
async fn pids_exact(name: &str) -> Vec<u32> {
    parse_pids(output_of("pgrep", &["-x", name]).await)
}

/// PIDs whose full command line matches a pattern (`pgrep -f`).
async fn pids_full(pattern: &str) -> Vec<u32> {
    parse_pids(output_of("pgrep", &["-f", pattern]).await)
}

fn parse_pids(out: Option<String>) -> Vec<u32> {
    out.unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

fn check_modules(r: &mut Report) {
    match std::fs::read_to_string("/sys/devices/evdi/count") {
        Ok(text) => {
            let count: i32 = text.trim().parse().unwrap_or(-1);
            if count > 0 {
                r.line(Level::Ok, "evdi module", &format!("{} device(s)", count));
            } else {
                r.line(Level::Fail, "evdi module", "loaded, but no device created");
                r.hint("echo 1 | sudo tee /sys/devices/evdi/add   (or run: make setup-system)");
            }
        }
        Err(_) => {
            r.line(Level::Fail, "evdi module", "not loaded");
            r.hint("sudo modprobe evdi   (install evdi-dkms if that fails)");
        }
    }

    let uinput = Path::new("/dev/uinput");
    if !uinput.exists() {
        r.line(Level::Fail, "uinput device", "/dev/uinput missing");
        r.hint("sudo modprobe uinput");
    } else {
        // Existence is not enough — the daemon runs unprivileged and needs to
        // open it for writing, which is what actually fails in practice.
        match std::fs::OpenOptions::new().write(true).open(uinput) {
            Ok(_) => r.line(Level::Ok, "uinput device", "writable"),
            Err(e) => {
                r.line(Level::Fail, "uinput device", &format!("not writable: {}", e));
                r.hint("add a udev rule granting your user access to /dev/uinput");
            }
        }
    }
}

async fn check_tools(r: &mut Report, cfg: &FileConfig) {
    for (tool, fatal) in [("ffmpeg", true), ("adb", true), ("kscreen-doctor", false)] {
        if command_exists(tool) {
            r.line(Level::Ok, tool, "found");
        } else if fatal {
            r.line(Level::Fail, tool, "not installed");
            r.hint(&format!("install {} with your package manager", tool));
        } else {
            r.line(Level::Warn, tool, "not installed (KDE only)");
        }
    }

    if let Some(list) = output_of("ffmpeg", &["-hide_banner", "-encoders"]).await {
        let has = |name: &str| list.lines().any(|l| l.split_whitespace().any(|t| t == name));
        if has(&cfg.encoder) {
            r.line(
                Level::Ok,
                "configured encoder",
                &format!("{} available", cfg.encoder),
            );
        } else {
            r.line(
                Level::Fail,
                "configured encoder",
                &format!("{} NOT available in ffmpeg", cfg.encoder),
            );
            let alternatives: Vec<&str> = ["h264_nvenc", "h264_vaapi", "libx264"]
                .into_iter()
                .filter(|e| has(e))
                .collect();
            r.hint(&format!("available instead: {}", alternatives.join(", ")));
        }
    }
}

/// The check that matters most: more than one helper or encoder means several
/// processes are writing/reading the same FIFO and every frame is corrupt.
async fn check_processes(r: &mut Report) {
    let daemons = pids_exact("uscreen").await;
    let helpers = pids_exact("evdi_helper").await;
    let encoders = pids_full(&format!("ffmpeg.*{}", FIFO_PATH)).await;

    // The PID file is the daemon's single slot; anything running beside it is
    // untracked and `uscreen stop` will never reach it.
    let pid_file = crate::get_pid_path();
    let tracked: Option<u32> = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .filter(|pid| Path::new(&format!("/proc/{}", pid)).exists());

    match tracked {
        Some(pid) => r.line(Level::Ok, "daemon", &format!("running, PID {}", pid)),
        None if pid_file.exists() => {
            r.line(Level::Warn, "daemon", "stale PID file, not running");
            r.hint(&format!("rm {}", pid_file.display()));
        }
        None => r.line(Level::Ok, "daemon", "not running"),
    }

    // `pgrep -x uscreen` also matches this very process — excluding it is not
    // cosmetic: reporting ourselves as an orphan would send the user off to
    // `pkill -x uscreen` chasing a process that never existed.
    let self_pid = std::process::id();
    let untracked: Vec<u32> = daemons
        .iter()
        .copied()
        .filter(|p| *p != self_pid && Some(*p) != tracked)
        .collect();
    if !untracked.is_empty() {
        r.line(
            Level::Fail,
            "orphaned uscreen daemons",
            &format!("{:?}", untracked),
        );
        r.hint("these fight over the same FIFO and ports — kill them: pkill -x uscreen");
    }

    if helpers.len() > 1 {
        r.line(
            Level::Fail,
            "evdi_helper processes",
            &format!("{} running: {:?}", helpers.len(), helpers),
        );
        r.hint("several writers interleave on the FIFO — torn frames: pkill -x evdi_helper");
    } else if helpers.len() == 1 && tracked.is_none() {
        r.line(Level::Fail, "evdi_helper", "orphaned (no daemon owns it)");
        r.hint("pkill -x evdi_helper");
    } else {
        r.line(
            Level::Ok,
            "evdi_helper processes",
            &format!("{}", helpers.len()),
        );
    }

    if encoders.len() > 1 {
        r.line(
            Level::Fail,
            "ffmpeg on capture FIFO",
            &format!("{} running: {:?}", encoders.len(), encoders),
        );
        r.hint("two readers on one pipe corrupt frames — kill the strays");
    } else if encoders.len() == 1 && tracked.is_none() {
        r.line(Level::Fail, "ffmpeg on capture FIFO", "orphaned");
        r.hint(&format!("pkill -f 'ffmpeg.*{}'", FIFO_PATH));
    } else {
        r.line(
            Level::Ok,
            "ffmpeg on capture FIFO",
            &format!("{}", encoders.len()),
        );
    }
}

async fn check_tablet(r: &mut Report, cfg: &FileConfig) {
    // `adb get-state` errors out when more than one device is attached, so
    // enumerate instead — that failure mode is silent otherwise.
    let Some(list) = output_of("adb", &["devices"]).await else {
        r.line(Level::Warn, "adb", "could not run");
        return;
    };
    let devices: Vec<&str> = list
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            (state == "device").then_some(serial)
        })
        .collect();

    // Which device every later check should address. With more than one
    // present, a bare `adb shell` fails outright ("more than one
    // device/emulator") — which is how this check used to report a perfectly
    // well installed app as missing the moment `adb tcpip` was in use.
    let chosen: Option<&str> = devices
        .iter()
        .find(|d| !d.contains(':'))
        .or_else(|| devices.first())
        .copied();

    match devices.len() {
        0 => {
            let unauthorized = list.contains("unauthorized");
            if unauthorized {
                r.line(Level::Fail, "tablet", "attached but unauthorized");
                r.hint("accept the 'Allow USB debugging' prompt on the tablet");
            } else {
                r.line(Level::Warn, "tablet", "not connected");
                r.hint("plug in the USB cable and enable USB debugging");
            }
            return;
        }
        1 => {
            r.line(Level::Ok, "tablet", devices[0]);
            report_transport(r, devices[0]);
        }
        n => {
            // Usually one tablet reachable two ways rather than two tablets:
            // `adb tcpip` leaves the cable working alongside the network
            // device. The daemon prefers the cable, so say which one wins.
            let pick = chosen.unwrap_or(devices[0]);
            r.line(
                Level::Ok,
                "tablet",
                &format!("{} reachable {} ways: {:?}", pick, n, devices),
            );
            report_transport(r, pick);
        }
    }
    let dev_args: Vec<String> = match chosen {
        Some(d) => vec!["-s".into(), d.to_string()],
        None => Vec::new(),
    };

    let mut reverse_args: Vec<&str> = dev_args.iter().map(|s| s.as_str()).collect();
    reverse_args.extend_from_slice(&["reverse", "--list"]);
    match output_of("adb", &reverse_args).await {
        Some(reverse) => {
            for port in [cfg.video_port, cfg.input_port] {
                let needle = format!("tcp:{}", port);
                if reverse.contains(&needle) {
                    r.line(Level::Ok, &format!("adb reverse {}", port), "forwarded");
                } else {
                    r.line(
                        Level::Warn,
                        &format!("adb reverse {}", port),
                        "not forwarded",
                    );
                    r.hint(&format!("adb reverse tcp:{p} tcp:{p}", p = port));
                }
            }
        }
        None => r.line(Level::Warn, "adb reverse", "could not query"),
    }

    let mut pm_args: Vec<&str> = dev_args.iter().map(|s| s.as_str()).collect();
    pm_args.extend_from_slice(&["shell", "pm", "list", "packages", "com.uscreen"]);
    if let Some(out) = output_of("adb", &pm_args).await {
        if out.contains("com.uscreen") {
            r.line(Level::Ok, "tablet app", "installed");
        } else {
            r.line(Level::Fail, "tablet app", "com.uscreen not installed");
            r.hint("adb install android/app/build/outputs/apk/debug/app-debug.apk");
        }
    }
}

async fn check_virtual_display(r: &mut Report, cfg: &FileConfig) {
    let connectors = vdisplay::evdi_connectors();
    if connectors.is_empty() {
        r.line(Level::Fail, "EVDI connector", "none found in sysfs");
        r.hint("the evdi module is loaded but exposes no DRM connector — reboot or re-add");
        return;
    }
    for c in &connectors {
        r.line(
            if c.connected { Level::Ok } else { Level::Warn },
            "EVDI connector",
            &format!(
                "{} ({})",
                c.name,
                if c.connected {
                    "connected"
                } else {
                    "disconnected — helper not running"
                }
            ),
        );
    }

    let names: Vec<&str> = connectors.iter().map(|c| c.name.as_str()).collect();
    let Some(json) = output_of("kscreen-doctor", &["-j"]).await else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
        r.line(Level::Warn, "kscreen-doctor", "unparseable JSON output");
        return;
    };
    let Some(outputs) = value.get("outputs").and_then(|o| o.as_array()) else {
        return;
    };

    for out in outputs {
        let name = out.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if !names.contains(&name) {
            continue;
        }
        let enabled = out
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            r.line(Level::Warn, "KDE output", &format!("{} is disabled", name));
            r.hint("the daemon enables it on start; nothing is rendered while it is off");
            continue;
        }
        let w = out.pointer("/size/width").and_then(|v| v.as_i64()).unwrap_or(0);
        let h = out
            .pointer("/size/height")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        // A mismatch here is the classic "skewed / torn picture": ffmpeg is
        // told one frame size while the helper produces another.
        if w as u32 == cfg.width && h as u32 == cfg.height {
            r.line(
                Level::Ok,
                "KDE output mode",
                &format!("{} at {}x{}", name, w, h),
            );
        } else {
            r.line(
                Level::Fail,
                "KDE output mode",
                &format!("{} is {}x{}, config says {}x{}", name, w, h, cfg.width, cfg.height),
            );
            r.hint("the encoder frame size will not match the capture — picture will be skewed");
        }
    }
}

/// The on-screen keyboard pops up whenever a touchscreen is used, and the
/// tablet's touch device is exactly that as far as the desktop is concerned.
/// Reported because it is a global desktop setting, not something the daemon
/// should quietly decide on the user's behalf.
async fn check_osk(r: &mut Report) {
    // The live value, not the one in kwinrc: KWin does not re-read that file,
    // so the two disagree routinely and only this one reflects what happens.
    let Some(raw) = output_of(
        "qdbus",
        &[
            "--literal",
            "org.kde.KWin",
            "/VirtualKeyboard",
            "org.freedesktop.DBus.Properties.Get",
            "org.kde.kwin.VirtualKeyboard",
            "mode",
        ],
    )
    .await
    else {
        return;
    };
    let mode: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    match mode.trim() {
        "1" | "2" => {
            r.line(
                Level::Warn,
                "on-screen keyboard",
                "pops up on touch input",
            );
            r.hint("the daemon turns this off while it runs and puts it back on exit");
        }
        "0" => r.line(Level::Ok, "on-screen keyboard", "only when asked for"),
        _ => {}
    }
}

/// Whether plugging the cable in is actually enough on its own.
async fn check_autostart(r: &mut Report) {
    match output_of("systemctl", &["--user", "is-enabled", "uscreen.service"]).await {
        Some(v) if v.trim() == "enabled" => {
            r.line(Level::Ok, "start with the desktop", "enabled");
        }
        Some(_) => {
            r.line(
                Level::Warn,
                "start with the desktop",
                "disabled — the daemon must be started by hand",
            );
            r.hint("systemctl --user enable --now uscreen.service");
        }
        None => {}
    }
}

/// Colour accuracy, for using the tablet to judge images rather than just to
/// hold windows. Everything here is a setting rather than a bug, but each one
/// silently ruins colour and none of them is visible from the host side.
async fn check_colour(r: &mut Report) {
    // Eye comfort / blue light filter warms the whole panel. Nothing on the
    // host can compensate, and it is easy to leave on by accident.
    match output_of("adb", &["shell", "settings", "get", "system", "blue_light_filter"]).await {
        Some(v) if v.trim() == "1" => {
            r.line(Level::Fail, "blue light filter", "ON — colours are warmed");
            r.hint("tablet: Settings → Display → Eye comfort shield → off");
        }
        Some(v) if v.trim() == "0" => r.line(Level::Ok, "blue light filter", "off"),
        _ => {}
    }

    // Samsung's "Vivid" screen mode stretches saturation past sRGB. "Natural"
    // is the colour-accurate one.
    if let Some(v) = output_of("adb", &["shell", "settings", "get", "system", "screen_mode_setting"]).await
    {
        let v = v.trim().to_string();
        if v == "2" {
            r.line(Level::Ok, "tablet screen mode", "Natural (sRGB)");
        } else if !v.is_empty() && v != "null" {
            r.line(
                Level::Warn,
                "tablet screen mode",
                &format!("{} — not the sRGB-accurate mode", v),
            );
            r.hint("tablet: Settings → Display → Screen mode → Natural");
        }
    }

    // KWin can colour-manage the virtual output, but only once a profile is
    // attached to it.
    let names: Vec<String> = vdisplay::evdi_connectors()
        .into_iter()
        .map(|c| c.name)
        .collect();
    if let Some(json) = output_of("kscreen-doctor", &["-j"]).await {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
            for out in v.get("outputs").and_then(|o| o.as_array()).unwrap_or(&vec![]) {
                let name = out.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !names.iter().any(|n| n == name) {
                    continue;
                }
                let icc = out
                    .get("iccProfilePath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if icc.is_empty() {
                    r.line(Level::Warn, "colour profile", "none assigned to the virtual display");
                    r.hint(
                        "System Settings → Display → pick the UScreen display → Color Profile. \
                         A generic sRGB profile is the right baseline; a measured one needs a \
                         colorimeter pointed at the tablet.",
                    );
                } else {
                    r.line(Level::Ok, "colour profile", icc);
                }
            }
        }
    }
}

fn check_config(r: &mut Report, cfg: &FileConfig) {
    let path = config::config_path();
    // `cfg` has already been clamped, so compare against the raw file too: a
    // stale 200 Mbps on disk is worth reporting even though the daemon would
    // no longer act on it.
    let on_disk: Option<FileConfig> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok());
    r.line(Level::Ok, "config file", &format!("{}", path.display()));
    r.line(
        Level::Ok,
        "screen position",
        match cfg.position.as_str() {
            "left" => "left of the other screens",
            "above" => "above the other screens",
            "below" => "below the other screens",
            _ => "right of the other screens",
        },
    );
    r.line(
        Level::Ok,
        "mode",
        if cfg.pen_only {
            "graphics tablet — no display is streamed (switchable from the tablet)"
        } else {
            "second screen (switchable from the tablet)"
        },
    );

    let raw_bitrate = on_disk.as_ref().map(|c| c.bitrate).unwrap_or(cfg.bitrate);
    if raw_bitrate > MAX_BITRATE_KBPS {
        r.line(
            Level::Warn,
            "bitrate on disk",
            &format!("{} Mbps — clamped to {} Mbps at runtime",
                raw_bitrate / 1000,
                cfg.bitrate / 1000),
        );
        r.hint("rewrite it via uscreen-gui (or the tablet settings) to make the file agree");
    } else {
        r.line(
            Level::Ok,
            "bitrate",
            &format!("{} Mbps", cfg.bitrate as f64 / 1000.0),
        );
    }

    let raw_fps = on_disk.as_ref().map(|c| c.fps).unwrap_or(cfg.fps);
    if raw_fps > MAX_FPS {
        r.line(
            Level::Warn,
            "fps on disk",
            &format!("{} — clamped to {} at runtime", raw_fps, cfg.fps),
        );
        r.hint("the generated EDID is capped at 90 Hz, so anything above is duplicate frames");
    } else {
        r.line(Level::Ok, "fps", &format!("{}", cfg.fps));
    }

    r.line(
        Level::Ok,
        "resolution",
        &format!(
            "{}x{}{}",
            cfg.width,
            cfg.height,
            if cfg.auto_resolution {
                " (auto, follows the tablet)"
            } else {
                " (fixed)"
            }
        ),
    );
}

pub async fn run() -> Result<()> {
    println!("=== uscreen doctor ===");

    let mut r = Report::new();
    // Loaded once: every load logs a warning when it clamps a stale value, and
    // repeating that between report sections buries the report itself.
    let cfg = FileConfig::load();

    section("Kernel modules");
    check_modules(&mut r);

    section("Tools");
    check_tools(&mut r, &cfg).await;

    section("Processes");
    check_processes(&mut r).await;

    section("Tablet");
    check_tablet(&mut r, &cfg).await;

    section("Virtual display");
    check_virtual_display(&mut r, &cfg).await;

    section("Autostart");
    check_autostart(&mut r).await;

    section("Desktop");
    check_osk(&mut r).await;

    section("Colour");
    check_colour(&mut r).await;

    section("Configuration");
    check_config(&mut r, &cfg);

    println!();
    if r.failures > 0 {
        println!(
            "{} problem(s) and {} warning(s) found — fix the FAIL lines first.",
            r.failures, r.warnings
        );
    } else if r.warnings > 0 {
        println!("No blocking problems, {} warning(s).", r.warnings);
    } else {
        println!("Everything checks out.");
    }
    Ok(())
}

/// A network serial is `host:port`; a USB serial never contains a colon.
/// Worth reporting because the two transports differ by far more than the
/// median suggests — the Wi-Fi tail is several times worse.
fn report_transport(r: &mut Report, serial: &str) {
    if serial.contains(':') {
        r.line(Level::Warn, "transport", "Wi-Fi — expect occasional stutter");
        r.hint("plug the USB cable in for steady latency; the daemon prefers it automatically");
    } else {
        r.line(Level::Ok, "transport", "USB");
    }
}
