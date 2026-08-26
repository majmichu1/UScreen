#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mirror of the daemon's config file (~/.config/uscreen/config.toml).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
struct FileConfig {
    encoder: String,
    fps: u32,
    bitrate: u32,
    width: u32,
    height: u32,
    quality: u32,
    stream_scale: u32,
    pen_only: bool,
    /// Which side of the desktop the virtual screen sits on. Kept here even
    /// though this window does not need it for anything else: the GUI writes
    /// the whole file back, so a field it does not know about is a field it
    /// silently erases.
    position: String,
    ten_bit: bool,
    auto_resolution: bool,
    video_port: u16,
    input_port: u16,
    auto_launch_app: bool,
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
            auto_resolution: true,
            video_port: 8890,
            input_port: 8891,
            auto_launch_app: true,
        }
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/uscreen/config.toml")
}

// Kept in sync with `host/src/config.rs`. TODO: share that module instead of
// mirroring it here — the duplication is what let these drift in the first place.
const MAX_BITRATE_KBPS: u32 = 60_000;
const MIN_BITRATE_KBPS: u32 = 1_000;
const MAX_FPS: u32 = 90;
const MIN_FPS: u32 = 10;
const DEFAULT_QUALITY: u32 = 18;
const MIN_QUALITY: u32 = 12;
const MAX_QUALITY: u32 = 32;

impl FileConfig {
    fn load() -> Self {
        let mut cfg: Self = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        // Older installs persisted 200 Mbps / 90 fps pushed from the tablet.
        // Without this the GUI would display that value and write it straight
        // back on the next save.
        cfg.bitrate = cfg.bitrate.clamp(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS);
        cfg.fps = cfg.fps.clamp(MIN_FPS, MAX_FPS);
        cfg.quality = cfg.quality.clamp(MIN_QUALITY, MAX_QUALITY);
        cfg.stream_scale = cfg.stream_scale.clamp(1, 4);
        cfg
    }

    fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self).unwrap_or_default())
    }
}

/// Whether the systemd user service is enabled, i.e. whether plugging the
/// cable in is enough on its own.
fn autostart_enabled() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", "uscreen.service"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "enabled")
        .unwrap_or(false)
}

fn set_autostart(on: bool) -> Result<(), String> {
    let verb = if on { "enable" } else { "disable" };
    let out = Command::new("systemctl")
        .args(["--user", verb, "--now", "uscreen.service"])
        .output()
        .map_err(|e| format!("systemctl failed: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[derive(Default, Clone)]
struct Status {
    daemon_running: bool,
    daemon_pid: u32,
    tablet_connected: bool,
    tablet_model: String,
    /// -1 = evdi module not loaded, otherwise the device count
    evdi_count: i32,
    ffmpeg_ok: bool,
    adb_ok: bool,
    autostart: bool,
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn pid_path() -> PathBuf {
    PathBuf::from(format!("{}/.local/share/uscreen/uscreen.pid", home()))
}

fn find_uscreen_bin() -> Option<PathBuf> {
    let installed = PathBuf::from(format!("{}/.local/bin/uscreen", home()));
    if installed.exists() {
        return Some(installed);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("uscreen");
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }
    // Fall back to PATH
    if Command::new("uscreen").arg("--version").output().is_ok() {
        return Some(PathBuf::from("uscreen"));
    }
    None
}

fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn poll_status() -> Status {
    let mut s = Status::default();

    s.evdi_count = std::fs::read_to_string("/sys/devices/evdi/count")
        .ok()
        .and_then(|t| t.trim().parse::<i32>().ok())
        .unwrap_or(-1);
    s.ffmpeg_ok = command_exists("ffmpeg");
    s.autostart = autostart_enabled();
    s.adb_ok = command_exists("adb");

    if let Ok(pid_str) = std::fs::read_to_string(pid_path()) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if PathBuf::from(format!("/proc/{}", pid)).exists() {
                s.daemon_running = true;
                s.daemon_pid = pid;
            }
        }
    }

    if let Ok(out) = Command::new("adb").arg("get-state").output() {
        s.tablet_connected = String::from_utf8_lossy(&out.stdout).trim() == "device";
    }
    if s.tablet_connected {
        if let Ok(out) = Command::new("adb").args(["devices", "-l"]).output() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines().skip(1) {
                if let Some(model) = line
                    .split_whitespace()
                    .find_map(|tok| tok.strip_prefix("model:"))
                {
                    s.tablet_model = model.replace('_', " ");
                    break;
                }
            }
        }
    }
    s
}

/// One-time privileged setup via the desktop's graphical password prompt:
/// pre-create an EVDI device now and at every boot.
fn run_system_setup() -> Result<(), String> {
    let script = "set -e; \
        echo 'options evdi initial_device_count=1' > /etc/modprobe.d/uscreen-evdi.conf; \
        printf 'evdi\nuinput\n' > /etc/modules-load.d/uscreen.conf; \
        modprobe evdi || true; modprobe uinput || true; \
        if [ \"$(cat /sys/devices/evdi/count 2>/dev/null || echo 0)\" = \"0\" ]; then echo 1 > /sys/devices/evdi/add; fi";
    let out = Command::new("pkexec")
        .args(["sh", "-c", script])
        .output()
        .map_err(|e| format!("pkexec failed to run: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Setup failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn start_daemon() -> Result<(), String> {
    let bin = find_uscreen_bin().ok_or("uscreen binary not found — run `make install`")?;
    let log_dir = PathBuf::from(format!("{}/.local/share/uscreen", home()));
    let _ = std::fs::create_dir_all(&log_dir);
    let log = std::fs::File::create(log_dir.join("daemon.log")).map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new(bin)
        .arg("start")
        .stdout(log)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start daemon: {}", e))?;
    // The daemon detaches and keeps running in the background, but the
    // std::process::Child handle must still be waited on or the kernel
    // leaves a zombie behind once it exits. Reap it on a background thread
    // instead of blocking the GUI.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn stop_daemon() -> Result<(), String> {
    let bin = find_uscreen_bin().ok_or("uscreen binary not found")?;
    Command::new(bin)
        .arg("stop")
        .output()
        .map_err(|e| format!("Failed to stop daemon: {}", e))?;
    Ok(())
}

struct App {
    cfg: FileConfig,
    saved_cfg: FileConfig,
    status: Arc<Mutex<Status>>,
    message: String,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = FileConfig::load();
        let status = Arc::new(Mutex::new(Status::default()));

        // Background poller: daemon + adb state every 2 seconds
        let status_bg = status.clone();
        std::thread::spawn(move || loop {
            let s = poll_status();
            if let Ok(mut guard) = status_bg.lock() {
                *guard = s;
            }
            std::thread::sleep(Duration::from_secs(2));
        });

        Self {
            saved_cfg: cfg.clone(),
            cfg,
            status,
            message: String::new(),
        }
    }

    fn apply(&mut self, restart: bool) {
        match self.cfg.save() {
            Ok(_) => {
                self.saved_cfg = self.cfg.clone();
                self.message = "Settings saved".into();
                if restart {
                    let _ = stop_daemon();
                    std::thread::sleep(Duration::from_millis(500));
                    match start_daemon() {
                        Ok(_) => self.message = "Settings saved — daemon restarted".into(),
                        Err(e) => self.message = e,
                    }
                }
            }
            Err(e) => self.message = format!("Save failed: {}", e),
        }
    }
}

fn scale_label(n: u32) -> &'static str {
    match n {
        3 => "Third — very soft",
        _ => "Quarter — very soft",
    }
}

fn status_dot(ui: &mut egui::Ui, on: bool, label: &str, detail: &str) {
    ui.horizontal(|ui| {
        let color = if on {
            egui::Color32::from_rgb(76, 175, 80)
        } else {
            egui::Color32::from_rgb(120, 120, 130)
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(egui::RichText::new(label).strong());
        if !detail.is_empty() {
            ui.label(egui::RichText::new(detail).weak());
        }
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        let status = self.status.lock().map(|s| s.clone()).unwrap_or_default();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading(egui::RichText::new("UScreen").size(26.0));
            ui.label(egui::RichText::new("USB second display for your tablet").weak());
            ui.add_space(12.0);

            // ----- First-run system setup -----
            let needs_setup = status.evdi_count <= 0;
            let missing_pkgs = !status.ffmpeg_ok || !status.adb_ok;
            if needs_setup || missing_pkgs {
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(50, 38, 22))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            egui::RichText::new("Setup needed")
                                .strong()
                                .color(egui::Color32::from_rgb(255, 180, 80)),
                        );
                        if missing_pkgs {
                            let mut pkgs = vec![];
                            if !status.ffmpeg_ok {
                                pkgs.push("ffmpeg");
                            }
                            if !status.adb_ok {
                                pkgs.push("android-tools (adb)");
                            }
                            ui.label(format!(
                                "Install with your package manager: {}",
                                pkgs.join(", ")
                            ));
                        }
                        if needs_setup {
                            ui.label(if status.evdi_count < 0 {
                                "The EVDI kernel module is not loaded (install evdi/evdi-dkms)."
                            } else {
                                "The virtual display device needs to be enabled (one time)."
                            });
                            if ui.button("Enable virtual display (asks for password)").clicked()
                            {
                                match run_system_setup() {
                                    Ok(_) => self.message = "System setup complete".into(),
                                    Err(e) => self.message = e,
                                }
                            }
                        }
                    });
                ui.add_space(10.0);
            }

            // ----- Status -----
            egui::Frame::group(ui.style())
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    status_dot(
                        ui,
                        status.daemon_running,
                        if status.daemon_running { "Daemon running" } else { "Daemon stopped" },
                        &if status.daemon_running {
                            format!("PID {}", status.daemon_pid)
                        } else {
                            String::new()
                        },
                    );
                    ui.add_space(4.0);
                    status_dot(
                        ui,
                        status.tablet_connected,
                        if status.tablet_connected { "Tablet connected" } else { "No tablet detected" },
                        &status.tablet_model,
                    );
                    if !status.tablet_connected {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "Plug in via USB and enable USB debugging on the tablet",
                            )
                            .weak()
                            .size(11.0),
                        );
                    }
                });

            ui.add_space(10.0);

            // ----- Start / Stop -----
            ui.horizontal(|ui| {
                let big = egui::vec2(ui.available_width(), 34.0);
                if status.daemon_running {
                    if ui
                        .add_sized(big, egui::Button::new(egui::RichText::new("Stop").size(16.0)))
                        .clicked()
                    {
                        match stop_daemon() {
                            Ok(_) => self.message = "Daemon stopped".into(),
                            Err(e) => self.message = e,
                        }
                    }
                } else if ui
                    .add_sized(big, egui::Button::new(egui::RichText::new("Start").size(16.0)))
                    .clicked()
                {
                    match start_daemon() {
                        Ok(_) => self.message = "Daemon starting…".into(),
                        Err(e) => self.message = e,
                    }
                }
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);

            // ----- Settings -----
            ui.label(egui::RichText::new("Settings").strong().size(15.0));
            ui.add_space(8.0);

            egui::Grid::new("settings")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Encoder");
                    egui::ComboBox::from_id_salt("encoder")
                        .selected_text(match self.cfg.encoder.as_str() {
                            "h264_nvenc" => "NVIDIA H.264 (NVENC)",
                            "hevc_nvenc" => "NVIDIA HEVC (NVENC)",
                            "h264_vaapi" | "vaapih264enc" => "AMD / Intel (VAAPI)",
                            "libx264" => "CPU (libx264)",
                            other => other,
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.cfg.encoder,
                                "h264_nvenc".to_string(),
                                "NVIDIA H.264 (NVENC)",
                            );
                            ui.selectable_value(
                                &mut self.cfg.encoder,
                                "hevc_nvenc".to_string(),
                                "NVIDIA HEVC (NVENC)",
                            );
                            ui.selectable_value(
                                &mut self.cfg.encoder,
                                "h264_vaapi".to_string(),
                                "AMD / Intel (VAAPI)",
                            );
                            ui.selectable_value(
                                &mut self.cfg.encoder,
                                "libx264".to_string(),
                                "CPU (libx264)",
                            );
                        });
                    ui.end_row();

                    ui.label("Quality");
                    ui.vertical(|ui| {
                        // Shown the intuitive way round — dragging right means
                        // sharper — while the stored value is the encoder's
                        // quantiser, where lower is better.
                        let mut sharpness =
                            (MAX_QUALITY - self.cfg.quality.clamp(MIN_QUALITY, MAX_QUALITY)) as f32;
                        let span = (MAX_QUALITY - MIN_QUALITY) as f32;
                        if ui
                            .add(egui::Slider::new(&mut sharpness, 0.0..=span).show_value(false))
                            .changed()
                        {
                            self.cfg.quality = MAX_QUALITY - sharpness.round() as u32;
                        }
                        ui.label(
                            egui::RichText::new(
                                "This, not the bitrate, sets how sharp text looks",
                            )
                            .weak()
                            .size(11.0),
                        );
                    });
                    ui.end_row();

                    ui.label("Bitrate ceiling");
                    ui.vertical(|ui| {
                        let mut mbps = self.cfg.bitrate as f32 / 1000.0;
                        // Capped at what the USB transport actually sustains:
                        // past that the encoder just outruns the link and the
                        // extra bits turn into queueing delay, not sharpness.
                        if ui
                            .add(egui::Slider::new(&mut mbps, 5.0..=60.0).suffix(" Mbps"))
                            .changed()
                        {
                            self.cfg.bitrate = (mbps * 1000.0) as u32;
                        }
                        ui.label(
                            egui::RichText::new(
                                "Only a cap for bursts — a desktop streams well below it",
                            )
                            .weak()
                            .size(11.0),
                        );
                    });
                    ui.end_row();

                    ui.label("Frame rate");
                    // 120 is not offered: EDID 1.4 stores the pixel clock in 16
                    // bits and 2960x1848@120 overflows it, so the virtual mode
                    // is capped at 90 Hz.
                    egui::ComboBox::from_id_salt("fps")
                        .selected_text(format!("{} fps", self.cfg.fps))
                        .show_ui(ui, |ui| {
                            for f in [30u32, 60, 90] {
                                ui.selectable_value(&mut self.cfg.fps, f, format!("{} fps", f));
                            }
                        });
                    ui.end_row();

                    ui.label("Position");
                    egui::ComboBox::from_id_salt("position")
                        .selected_text(match self.cfg.position.as_str() {
                            "left" => "Left of the other screens",
                            "above" => "Above the other screens",
                            "below" => "Below the other screens",
                            _ => "Right of the other screens",
                        })
                        .show_ui(ui, |ui| {
                            for (v, label) in [
                                ("right", "Right of the other screens"),
                                ("left", "Left of the other screens"),
                                ("above", "Above the other screens"),
                                ("below", "Below the other screens"),
                            ] {
                                ui.selectable_value(&mut self.cfg.position, v.to_string(), label);
                            }
                        });
                    ui.end_row();

                    ui.label("Colour depth");
                    ui.vertical(|ui| {
                        let hevc = self.cfg.encoder.contains("hevc");
                        ui.add_enabled(
                            hevc,
                            egui::Checkbox::new(&mut self.cfg.ten_bit, "10-bit (HEVC Main10)"),
                        );
                        ui.label(
                            egui::RichText::new(if hevc {
                                "Smooths banding on gradients. The desktop itself is 8-bit, \
                                 so this adds precision, not colour."
                            } else {
                                "Needs the HEVC encoder — H.264 here is 8-bit only."
                            })
                            .small()
                            .weak(),
                        );
                    });
                    ui.end_row();

                    ui.label("Resolution");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut self.cfg.auto_resolution, "Auto (match the tablet)");
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                !self.cfg.auto_resolution,
                                egui::DragValue::new(&mut self.cfg.width)
                                    .range(640..=8192)
                                    .speed(8),
                            );
                            ui.label("×");
                            ui.add_enabled(
                                !self.cfg.auto_resolution,
                                egui::DragValue::new(&mut self.cfg.height)
                                    .range(480..=8192)
                                    .speed(8),
                            );
                        });
                        if self.cfg.auto_resolution {
                            ui.label(
                                egui::RichText::new(format!(
                                    "currently {} × {}",
                                    self.cfg.width, self.cfg.height
                                ))
                                .weak()
                                .size(11.0),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label("Mode");
                    ui.vertical(|ui| {
                        ui.checkbox(
                            &mut self.cfg.pen_only,
                            "Graphics tablet instead of a second screen",
                        );
                        ui.label(
                            egui::RichText::new(
                                "Nothing is streamed: the pen drives this machine's own \
                                 screen, so there is no display latency at all. Pressure, \
                                 tilt and the eraser still work.",
                            )
                            .weak()
                            .size(11.0),
                        );
                    });
                    ui.end_row();

                    ui.label("Stream detail");
                    ui.vertical(|ui| {
                        egui::ComboBox::from_id_salt("stream_scale")
                            .selected_text(match self.cfg.stream_scale {
                                1 => "Full — sharpest",
                                2 => "Half — lowest latency",
                                n => scale_label(n),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.cfg.stream_scale, 1,
                                    "Full — sharpest");
                                ui.selectable_value(&mut self.cfg.stream_scale, 2,
                                    "Half — lowest latency");
                            });
                        ui.label(
                            egui::RichText::new(
                                "The desktop keeps its full resolution either way. Half sends \
                                 a quarter of the pixels, which the tablet decodes sooner — \
                                 good for games, softer for text.",
                            )
                            .weak()
                            .size(11.0),
                        );
                    });
                    ui.end_row();

                    ui.label("Plug & play");
                    ui.vertical(|ui| {
                        ui.checkbox(
                            &mut self.cfg.auto_launch_app,
                            "Open the app on the tablet automatically",
                        );
                        let mut auto = status.autostart;
                        if ui
                            .checkbox(&mut auto, "Start UScreen with the desktop")
                            .changed()
                        {
                            match set_autostart(auto) {
                                Ok(_) => {
                                    self.message = if auto {
                                        "Autostart on — plugging the cable in is now enough".into()
                                    } else {
                                        "Autostart off".into()
                                    }
                                }
                                Err(e) => self.message = e,
                            }
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(12.0);

            let dirty = self.cfg != self.saved_cfg;
            ui.horizontal(|ui| {
                let label = if status.daemon_running {
                    "Apply & restart"
                } else {
                    "Save"
                };
                if ui
                    .add_enabled(dirty, egui::Button::new(label))
                    .clicked()
                {
                    self.apply(status.daemon_running);
                }
                if dirty && ui.button("Discard").clicked() {
                    self.cfg = self.saved_cfg.clone();
                }
            });

            if !self.message.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&self.message).weak());
            }

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label(
                    egui::RichText::new(format!("config: {}", config_path().display()))
                        .weak()
                        .size(10.0),
                );
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 600.0])
            .with_min_inner_size([360.0, 520.0])
            .with_app_id("uscreen-gui"),
        ..Default::default()
    };
    eframe::run_native(
        "UScreen",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
