mod capture;
mod config;
mod edid;
mod input;
mod screencap;
mod stream;
mod vdisplay;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::signal;
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(
    name = "uscreen",
    version,
    about = "USB second-screen server for Linux"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long = "display")]
    display: Option<String>,

    /// Explicit EDID override. By default an EDID is generated at runtime
    /// for the configured (or tablet-reported) resolution.
    #[arg(long = "edid")]
    edid: Option<PathBuf>,

    #[arg(long = "helper", default_value = "host/evdi/evdi_helper")]
    helper: PathBuf,

    #[arg(long = "auto-vdisplay", default_value_t = true)]
    auto_vdisplay: bool,

    /// Defaults come from ~/.config/uscreen/config.toml; CLI flags override.
    #[arg(long = "encoder")]
    encoder: Option<String>,

    #[arg(long = "fps")]
    fps: Option<u32>,

    #[arg(long = "bitrate")]
    bitrate: Option<u32>,

    #[arg(long = "width")]
    width: Option<u32>,

    #[arg(long = "height")]
    height: Option<u32>,

    #[arg(long = "capture-mode", default_value = "evdi")]
    capture_mode: String,

    #[arg(long = "screen-name", default_value = "DVI-I-9")]
    screen_name: String,

    #[arg(long = "video-port")]
    video_port: Option<u16>,

    #[arg(long = "input-port")]
    input_port: Option<u16>,

    #[arg(long = "tablet-width", default_value_t = 2960)]
    tablet_width: u32,

    #[arg(long = "tablet-height", default_value_t = 1848)]
    tablet_height: u32,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the uscreen daemon
    Start {
        #[arg(long = "daemon", short = 'd')]
        daemonize: bool,
    },
    /// Stop the uscreen daemon
    Stop,
    /// Show daemon status
    Status,
    /// List available displays
    ListDisplays,
    /// Setup virtual display
    SetupVdisplay {
        #[arg(long = "connector")]
        connector: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    setup_logging();

    match &cli.command {
        Some(Commands::Start { .. }) | None => {
            info!("Starting uscreen daemon");
            run_daemon(cli).await?;
        }
        Some(Commands::Stop) => stop_daemon().await?,
        Some(Commands::Status) => show_status().await?,
        Some(Commands::ListDisplays) => list_displays().await?,
        Some(Commands::SetupVdisplay { connector }) => {
            setup_virtual_display(connector.as_deref()).await?;
        }
    }

    Ok(())
}

fn setup_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uscreen=info".into()),
        )
        .with_target(true)
        .with_line_number(true)
        .init();
}

async fn run_daemon(cli: Cli) -> Result<()> {
    // Write PID file for clean stop/status
    let pid_path = get_pid_path();
    let pid = std::process::id();
    if let Some(parent) = pid_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&pid_path, pid.to_string())?;

    // Settings precedence: CLI flag > config file > built-in default
    let file_cfg = config::FileConfig::load();
    let encoder = cli.encoder.clone().unwrap_or(file_cfg.encoder.clone());
    let fps = cli.fps.unwrap_or(file_cfg.fps);
    let bitrate = cli.bitrate.unwrap_or(file_cfg.bitrate);
    let width = cli.width.unwrap_or(file_cfg.width);
    let height = cli.height.unwrap_or(file_cfg.height);
    let video_port = cli.video_port.unwrap_or(file_cfg.video_port);
    let input_port = cli.input_port.unwrap_or(file_cfg.input_port);

    let cap_config = capture::CaptureConfig {
        helper_path: find_helper(&cli.helper),
        edid_path: cli.edid.clone(),
        encoder: encoder.clone(),
        fps,
        bitrate,
        width,
        height,
        capture_mode: cli.capture_mode.clone(),
        screen_name: cli.screen_name.clone(),
    };

    let stream_config = stream::StreamConfig { video_port };

    let input_config = input::InputConfig {
        port: input_port,
        virtual_width: width,
        virtual_height: height,
        tablet_width: cli.tablet_width,
        tablet_height: cli.tablet_height,
    };

    // Live-tunable settings (from the tablet app or by editing the config
    // file). A change restarts the encoder on the fly.
    let (settings_tx, settings_rx) = watch::channel(capture::EncoderSettings {
        encoder: encoder.clone(),
        fps,
        bitrate,
        width,
        height,
    });

    let mut capture_mgr = capture::CaptureManager::new(cap_config);
    let codec_config = capture_mgr.codec_config_arc();
    let stream_srv = stream::StreamServer::new(stream_config, codec_config);
    let input_srv = input::InputServer::new(input_config, Some(settings_tx.clone()));

    let (video_tx, _) = broadcast::channel(256);

    info!("=== uscreen daemon starting ===");
    info!("  Resolution: {}x{} @ {}fps", width, height, fps);
    info!("  Encoder: {}", encoder);
    info!("  Bitrate: {} kbps", bitrate);
    info!("  Stream port: {}", video_port);
    info!("  Input port: {}", input_port);

    let video_tx_cap = video_tx.clone();
    let settings_rx_cap = settings_rx.clone();
    let cap_handle = tokio::spawn(async move {
        if let Err(e) = capture_mgr.stream_frames(video_tx_cap, settings_rx_cap).await {
            error!("Capture manager failed: {}", e);
        }
    });

    let video_rx = video_tx.subscribe();
    let stream_handle = tokio::spawn(async move {
        if let Err(e) = stream_srv.run(video_rx).await {
            error!("Stream server failed: {}", e);
        }
    });

    let input_handle = tokio::spawn(async move {
        if let Err(e) = input_srv.run().await {
            error!("Input server failed: {}", e);
        }
    });

    // Persist settings changes pushed at runtime back to the config file
    let mut settings_rx_save = settings_rx.clone();
    let save_handle = tokio::spawn(async move {
        while settings_rx_save.changed().await.is_ok() {
            let s = settings_rx_save.borrow().clone();
            let mut cfg = config::FileConfig::load();
            cfg.encoder = s.encoder;
            cfg.fps = s.fps;
            cfg.bitrate = s.bitrate;
            cfg.width = s.width;
            cfg.height = s.height;
            if let Err(e) = cfg.save() {
                warn!("Failed to persist settings: {}", e);
            } else {
                info!("Settings saved to {:?}", config::config_path());
            }
        }
    });

    // Plug-and-play: watch for the tablet over ADB, set up port forwarding
    // and launch the app whenever it's (re)connected.
    let auto_launch = file_cfg.auto_launch_app;
    let adb_handle = tokio::spawn(async move {
        adb_monitor(video_port, input_port, auto_launch).await;
    });

    println!();
    println!("================================================");
    println!("  uscreen daemon running (PID: {})", pid);
    println!("================================================");
    println!("  On your tablet, open the UScreen app");
    println!("  ADB ports will be auto-forwarded if possible.");
    println!("  Otherwise, run:");
    println!("    adb reverse tcp:8890 tcp:8890");
    println!("    adb reverse tcp:8891 tcp:8891");
    println!("================================================");
    println!();

    signal::ctrl_c().await?;
    info!("Shutting down...");

    cap_handle.abort();
    stream_handle.abort();
    input_handle.abort();
    adb_handle.abort();
    save_handle.abort();

    // Clean up PID file
    let _ = std::fs::remove_file(&pid_path);

    info!("uscreen daemon stopped");
    Ok(())
}

fn get_pid_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{}/.local/share/uscreen/uscreen.pid", home))
}

fn find_helper(path: &PathBuf) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();

    let installed = PathBuf::from(format!("{}/.local/bin/evdi_helper", home));
    if installed.exists() {
        return installed;
    }

    if path.exists() {
        if let Ok(canon) = path.canonicalize() {
            return canon;
        }
        return path.clone();
    }

    let alt = PathBuf::from("host/evdi/evdi_helper");
    if alt.exists() {
        if let Ok(canon) = alt.canonicalize() {
            return canon;
        }
        return alt;
    }

    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        for _ in 0..5 {
            if let Some(d) = dir {
                let from_exe = d.join("host").join("evdi").join("evdi_helper");
                if from_exe.exists() {
                    if let Ok(canon) = from_exe.canonicalize() {
                        return canon;
                    }
                    return from_exe;
                }
                dir = d.parent();
            } else {
                break;
            }
        }
    }

    path.clone()
}

/// Keeps watching for the tablet. On every (re)connect: set up reverse port
/// forwarding and optionally launch the UScreen app — plug in and it works.
async fn adb_monitor(video_port: u16, input_port: u16, auto_launch: bool) {
    let mut was_connected = false;
    loop {
        let connected = adb_device_ready().await;

        if connected && !was_connected {
            info!("Tablet connected via USB");
            match setup_adb_forwarding(video_port, input_port).await {
                Ok(_) => {
                    info!("ADB port forwarding set up ({}, {})", video_port, input_port);
                    if auto_launch {
                        let r = tokio::process::Command::new("adb")
                            .args([
                                "shell",
                                "am",
                                "start",
                                "-n",
                                "com.uscreen/.MainActivity",
                            ])
                            .output()
                            .await;
                        match r {
                            Ok(o) if o.status.success() => info!("UScreen app launched on tablet"),
                            _ => warn!("Could not auto-launch the app (is it installed?)"),
                        }
                    }
                }
                Err(e) => warn!("ADB forwarding failed: {}", e),
            }
        } else if !connected && was_connected {
            info!("Tablet disconnected");
        }

        was_connected = connected;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

async fn adb_device_ready() -> bool {
    match tokio::process::Command::new("adb")
        .args(["get-state"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "device",
        Err(_) => false,
    }
}

async fn setup_adb_forwarding(video_port: u16, input_port: u16) -> Result<()> {
    for port in [video_port, input_port] {
        let arg = format!("tcp:{}", port);
        let r = tokio::process::Command::new("adb")
            .args(["reverse", &arg, &arg])
            .output()
            .await?;
        if !r.status.success() {
            anyhow::bail!("adb reverse {} failed", arg);
        }
    }
    Ok(())
}

async fn stop_daemon() -> Result<()> {
    let pid_path = get_pid_path();

    if pid_path.exists() {
        let pid_str = std::fs::read_to_string(&pid_path)?;
        let pid: u32 = pid_str.trim().parse()?;
        info!("Stopping uscreen daemon (PID: {})", pid);

        let result = tokio::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await?;

        if result.status.success() {
            let _ = std::fs::remove_file(&pid_path);
            info!("uscreen daemon stopped");
        } else {
            // Fallback: try pkill but exclude our own PID
            let my_pid = std::process::id().to_string();
            tokio::process::Command::new("bash")
                .args([
                    "-c",
                    &format!(
                        "pgrep -f 'uscreen start' | grep -v {} | xargs -r kill -TERM",
                        my_pid
                    ),
                ])
                .output()
                .await?;
            let _ = std::fs::remove_file(&pid_path);
            info!("uscreen daemon stopped (fallback)");
        }
    } else {
        // No PID file, try pkill but exclude self
        let my_pid = std::process::id().to_string();
        tokio::process::Command::new("bash")
            .args([
                "-c",
                &format!(
                    "pgrep -f 'uscreen start' | grep -v {} | xargs -r kill -TERM",
                    my_pid
                ),
            ])
            .output()
            .await?;
        info!("uscreen daemon stopped (no PID file)");
    }

    Ok(())
}

async fn show_status() -> Result<()> {
    let pid_path = get_pid_path();
    let my_pid = std::process::id();

    if pid_path.exists() {
        let pid_str = std::fs::read_to_string(&pid_path)?;
        let pid: u32 = pid_str.trim().parse().unwrap_or(0);

        if pid > 0 && pid != my_pid {
            // Check if the process is actually running
            let proc_path = format!("/proc/{}", pid);
            if std::path::Path::new(&proc_path).exists() {
                println!("uscreen is running (PID: {})", pid);
            } else {
                println!("uscreen is not running (stale PID file)");
                let _ = std::fs::remove_file(&pid_path);
            }
        } else {
            println!("uscreen is not running");
        }
    } else {
        // Fallback: pgrep excluding self
        let out = tokio::process::Command::new("bash")
            .args([
                "-c",
                &format!("pgrep -f 'uscreen start' | grep -v {}", my_pid),
            ])
            .output()
            .await?;
        let pids = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !pids.is_empty() {
            println!("uscreen is running (PID: {})", pids);
        } else {
            println!("uscreen is not running");
        }
    }
    Ok(())
}

async fn list_displays() -> Result<()> {
    println!("=== Available displays ===");
    if let Ok(out) = tokio::process::Command::new("kscreen-doctor")
        .args(["-o"])
        .output()
        .await
    {
        println!("{}", String::from_utf8_lossy(&out.stdout));
    }

    if let Ok(out) = tokio::process::Command::new("wpctl")
        .args(["status"])
        .output()
        .await
    {
        println!("--- PipeWire ---");
        println!("{}", String::from_utf8_lossy(&out.stdout));
    }
    Ok(())
}

async fn setup_virtual_display(connector: Option<&str>) -> Result<()> {
    let _ = connector;
    println!("=== Virtual Display Setup ===");
    let helper_path = PathBuf::from("host/evdi/evdi_helper");
    let cfg = config::FileConfig::load();
    let edid_path = edid::ensure_edid(cfg.width, cfg.height, 60)?;
    let mut vd = vdisplay::VirtualDisplayManager::new(
        find_helper(&helper_path).as_path(),
        edid_path.as_path(),
    );
    match vd.create().await {
        Ok(name) => {
            println!("EVDI virtual display created: {}", name);
            println!("Press Ctrl+C to remove it.");
            signal::ctrl_c().await?;
        }
        Err(e) => {
            eprintln!("Failed to create virtual display: {}", e);
        }
    }
    Ok(())
}
