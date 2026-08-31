mod capture;
mod config;
mod doctor;
mod edid;
#[cfg(feature = "inproc-encoder")]
mod encoder;
mod input;
mod latency;
mod osk;
mod runtime;
mod stream;
mod tray;
mod update;
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

    #[arg(long = "quality")]
    quality: Option<u32>,

    /// Integer downscale for the stream only; the desktop keeps its native mode.
    #[arg(long = "stream-scale")]
    stream_scale: Option<u32>,

    /// Drive the laptop's own screen with the pen instead of streaming a second
    /// display to the tablet.
    #[arg(long = "pen-only")]
    pen_only: bool,

    #[arg(long = "video-port")]
    video_port: Option<u16>,

    #[arg(long = "input-port")]
    input_port: Option<u16>,
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
    /// Diagnose the whole setup and report what is wrong
    Doctor,
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
        Some(Commands::Doctor) => doctor::run().await?,
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
    let pid_path = get_pid_path();
    if let Some(parent) = pid_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Refuse to start a second daemon on top of a live one: the PID file is
    // a single slot, so `uscreen stop` only ever kills the most recently
    // started process — any earlier instance still running would become
    // permanently untracked, and both would keep writing/reading the same
    // EVDI FIFO, corrupting frames and starving the encoder.
    if let Ok(existing) = std::fs::read_to_string(&pid_path) {
        if let Ok(existing_pid) = existing.trim().parse::<i32>() {
            let alive = std::path::Path::new(&format!("/proc/{}", existing_pid)).exists();
            if alive {
                anyhow::bail!(
                    "uscreen daemon already running (PID: {}). Run `uscreen stop` first.",
                    existing_pid
                );
            }
        }
    }

    // Write PID file for clean stop/status
    let pid = std::process::id();
    std::fs::write(&pid_path, pid.to_string())?;

    // Settings precedence: CLI flag > config file > built-in default
    let file_cfg = config::FileConfig::load();

    // `load()` clamps unusable values, but leaving the bad number on disk means
    // the GUI keeps showing it and writes it straight back. Heal the file once,
    // here, so every tool agrees on what the settings actually are.
    {
        let raw: Option<config::FileConfig> = std::fs::read_to_string(config::config_path())
            .ok()
            .and_then(|t| toml::from_str(&t).ok());
        if raw.is_some_and(|r| r != file_cfg) {
            match file_cfg.save() {
                Ok(_) => info!("Rewrote out-of-range settings in {:?}", config::config_path()),
                Err(e) => warn!("Could not rewrite the config file: {}", e),
            }
        }
    }
    let encoder = cli.encoder.clone().unwrap_or(file_cfg.encoder.clone());
    let fps = cli.fps.unwrap_or(file_cfg.fps);
    let bitrate = cli.bitrate.unwrap_or(file_cfg.bitrate);
    let width = cli.width.unwrap_or(file_cfg.width);
    let height = cli.height.unwrap_or(file_cfg.height);
    let video_port = cli.video_port.unwrap_or(file_cfg.video_port);
    let input_port = cli.input_port.unwrap_or(file_cfg.input_port);
    let quality = cli.quality.unwrap_or(file_cfg.quality);
    let stream_scale = cli.stream_scale.unwrap_or(file_cfg.stream_scale);
    let pen_only = cli.pen_only || file_cfg.pen_only;

    let cap_config = capture::CaptureConfig {
        helper_path: find_helper(&cli.helper),
        edid_path: cli.edid.clone(),
        encoder: encoder.clone(),
        fps,
        bitrate,
        width,
        height,
        quality,
        // Replaced as soon as the tablet reports its real panel size.
        width_mm: edid::DEFAULT_WIDTH_MM,
        height_mm: edid::DEFAULT_HEIGHT_MM,
        stream_scale,
        position: config::Position::parse_or_default(&file_cfg.position),
        ten_bit: file_cfg.ten_bit,
    };

    // One secret per daemon run. Handed to the app over adb when it is
    // launched; anything connecting to the loopback ports without it gets
    // nothing. See config::FileConfig::require_token.
    let token: Option<String> = if file_cfg.require_token {
        match runtime::new_session_token() {
            Ok(t) => Some(t),
            Err(e) => {
                warn!("Could not create a session token ({}); running without one", e);
                None
            }
        }
    } else {
        warn!("require_token = false: any local process can read the screen and inject input");
        None
    };
    let relaunch = std::sync::Arc::new(tokio::sync::Notify::new());

    let stream_config = stream::StreamConfig { video_port, token: token.clone() };

    let input_config = input::InputConfig {
        port: input_port,
        token: token.clone(),
        codec: capture::Codec::from_encoder(&encoder).muxer().to_string(),
        virtual_width: width,
        virtual_height: height,
    };

    // Which of the two jobs the tablet is doing. Switchable at runtime from
    // the tablet's own settings, so it lives in a channel the input server,
    // the capture manager and the config writer all follow.
    let (mode_tx, _) = watch::channel(pen_only);

    // Live-tunable settings (from the tablet app or by editing the config
    // file). A change restarts the encoder on the fly.
    let (settings_tx, settings_rx) = watch::channel(capture::EncoderSettings {
        encoder: encoder.clone(),
        fps,
        bitrate,
        width,
        height,
        quality,
        width_mm: edid::DEFAULT_WIDTH_MM,
        height_mm: edid::DEFAULT_HEIGHT_MM,
        stream_scale,
    });

    let mut capture_mgr = capture::CaptureManager::new(cap_config);
    let codec_config = capture_mgr.codec_config_arc();
    let latency = capture_mgr.latency_tracker();
    let stream_srv =
        stream::StreamServer::new(stream_config, codec_config, capture_mgr.idr_request_flag());
    let input_srv = input::InputServer::new(
        input_config,
        Some(settings_tx.clone()),
        mode_tx.clone(),
        latency,
        relaunch.clone(),
    );

    // Deliberately shallow. This ring is pure latency when it fills: 256 frames
    // is four seconds of backlog at 60 fps, and a client that fell behind would
    // dutifully receive all of it instead of skipping to something current.
    // At 8, `RecvError::Lagged` fires early and the skip-to-newest-IDR path in
    // the stream server actually gets a chance to run.
    let (video_tx, _) = broadcast::channel(8);

    // The tablet is a touchscreen as far as the desktop is concerned, so the
    // virtual keyboard would pop up over the screen being used as a monitor.
    osk::disable().await;

    info!("=== uscreen daemon starting ===");
    info!("  Resolution: {}x{} @ {}fps", width, height, fps);
    info!("  Encoder: {}", encoder);
    info!("  Bitrate: {} kbps", bitrate);
    info!("  Stream port: {}", video_port);
    info!("  Input port: {}", input_port);

    // Tablet presence, published by the ADB monitor.
    let (tablet_tx, mut tablet_rx) = watch::channel(false);
    let tray_tablet_rx = tablet_tx.subscribe();

    // What the capture manager actually follows: the virtual display should
    // exist exactly while a tablet is attached *and* being used as a screen.
    // Folding the mode in here is what makes switching live — the capture
    // manager already knows how to raise and tear down the display on this
    // signal, and cannot tell the difference between an unplugged tablet and
    // one that is currently a drawing surface.
    let (gate_tx, gate_rx) = watch::channel(false);
    let mut mode_rx_gate = mode_tx.subscribe();
    tokio::spawn(async move {
        let mut last = false;
        loop {
            let active = *tablet_rx.borrow() && !*mode_rx_gate.borrow();
            if active != last {
                last = active;
                let _ = gate_tx.send(active);
            }
            tokio::select! {
                r = tablet_rx.changed() => if r.is_err() { break },
                r = mode_rx_gate.changed() => if r.is_err() { break },
            }
        }
    });

    // Cooperative shutdown: the capture task must get a chance to kill and reap
    // ffmpeg/evdi_helper before the process exits, or they linger holding the
    // capture FIFO and collide with the next start.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    if pen_only {
        info!("Starting in pen-only mode: the tablet drives this machine's own");
        info!("  screen with the pen. No virtual display, no encoding.");
    }

    let video_tx_cap = video_tx.clone();
    let settings_rx_cap = settings_rx.clone();
    let cap_handle = tokio::spawn(async move {
        // Started in both modes. In pen-only the gate below holds the virtual
        // output disabled and nothing is encoded, but the helper is up and
        // ready — which is what lets a switch back to second-screen mode show
        // a picture immediately instead of rebuilding the pipeline first.
        if let Err(e) = capture_mgr
            .stream_frames(video_tx_cap, settings_rx_cap, gate_rx, shutdown_rx)
            .await
        {
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

    // Fields the user overrode on the command line for this run only. They
    // must not be written back: a flag is not a settings change, and
    // persisting one silently rewrites the user's configuration behind them.
    let cli_overrides = (
        cli.encoder.is_some(),
        cli.fps.is_some(),
        cli.bitrate.is_some(),
        cli.width.is_some(),
        cli.height.is_some(),
        cli.quality.is_some(),
        cli.stream_scale.is_some(),
    );

    // Persist settings changes pushed at runtime back to the config file
    let mut settings_rx_save = settings_rx.clone();
    let save_handle = tokio::spawn(async move {
        while settings_rx_save.changed().await.is_ok() {
            let s = settings_rx_save.borrow().clone();
            let mut cfg = config::FileConfig::load();
            if !cli_overrides.0 { cfg.encoder = s.encoder; }
            if !cli_overrides.1 { cfg.fps = s.fps; }
            if !cli_overrides.2 { cfg.bitrate = s.bitrate; }
            if !cli_overrides.3 { cfg.width = s.width; }
            if !cli_overrides.4 { cfg.height = s.height; }
            if !cli_overrides.5 { cfg.quality = s.quality; }
            if !cli_overrides.6 { cfg.stream_scale = s.stream_scale; }
            if let Err(e) = cfg.save() {
                warn!("Failed to persist settings: {}", e);
            } else {
                info!("Settings saved to {:?}", config::config_path());
            }
        }
    });

    // Remember which mode the tablet was left in. Unlike the --pen-only flag,
    // which is a one-off for this run and never written back, a switch made
    // from the tablet is a deliberate choice and should survive a restart.
    let mut mode_rx_save = mode_tx.subscribe();
    let mode_save_handle = tokio::spawn(async move {
        while mode_rx_save.changed().await.is_ok() {
            let pen_only = *mode_rx_save.borrow();
            let mut cfg = config::FileConfig::load();
            cfg.pen_only = pen_only;
            if let Err(e) = cfg.save() {
                warn!("Failed to persist mode: {}", e);
            }
        }
    });

    // The daemon's only face on the desktop. It follows the same channels the
    // rest of the daemon does, so it cannot drift out of step with what is
    // actually running.
    // Once a day, ask GitHub whether there is a newer release. Reported in
    // the tray and by doctor; never installed from here.
    let (update_tx, update_rx) = watch::channel::<update::Available>(None);
    let update_handle = if file_cfg.check_updates {
        Some(tokio::spawn(async move { update::run(update_tx).await }))
    } else {
        None
    };

    let tray_mode_tx = mode_tx.clone();
    let tray_shutdown_tx = shutdown_tx.clone();
    let tray_handle = tokio::spawn(async move {
        tray::run(tray_mode_tx, tray_tablet_rx, tray_shutdown_tx, update_rx).await;
    });

    // Plug-and-play: watch for the tablet over ADB, set up port forwarding
    // and launch the app whenever it's (re)connected.
    let auto_launch = file_cfg.auto_launch_app;
    let adb_token = token.clone();
    let adb_handle = tokio::spawn(async move {
        adb_monitor(video_port, input_port, auto_launch, tablet_tx, adb_token, relaunch).await;
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

    // `uscreen stop` (and the GUI) send SIGTERM, not SIGINT — without a
    // handler for it, the kernel kills the process with its default
    // disposition and none of our cleanup (which is what kills the
    // evdi_helper/ffmpeg children via kill_on_drop) ever runs, orphaning
    // them to fight over the shared FIFO with the next daemon that starts.
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    // Quit from the tray raises the same flag the signal handlers do, so it
    // has to be waited on here too — otherwise the pipeline winds down while
    // the process itself stays alive with nothing left to run.
    let mut quit_rx = shutdown_tx.subscribe();
    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
        _ = async { while quit_rx.changed().await.is_ok() && !*quit_rx.borrow() {} } => {}
    }
    info!("Shutting down...");

    // Ask the capture pipeline to wind down, and give it a bounded moment to
    // actually reap its children before pulling the rug out.
    let _ = shutdown_tx.send(true);
    if tokio::time::timeout(std::time::Duration::from_secs(5), cap_handle)
        .await
        .is_err()
    {
        warn!("Capture pipeline did not stop within 5s");
    }

    osk::restore().await;

    stream_handle.abort();
    input_handle.abort();
    adb_handle.abort();
    save_handle.abort();
    mode_save_handle.abort();
    tray_handle.abort();
    if let Some(h) = update_handle {
        h.abort();
    }

    // Clean up PID file
    let _ = std::fs::remove_file(&pid_path);

    info!("uscreen daemon stopped");
    Ok(())
}

fn get_pid_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{}/.local/share/uscreen/uscreen.pid", home))
}

/// Resolve the helper binary.
///
/// An explicitly given `--helper` always wins. It used to lose to the copy in
/// `~/.local/bin`, so a freshly built helper was silently ignored in favour of
/// whatever was last installed — the kind of thing that costs an afternoon.
fn find_helper(path: &std::path::Path) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();

    if path.exists() {
        if let Ok(canon) = path.canonicalize() {
            return canon;
        }
        return path.to_path_buf();
    }

    // Wherever an installer may have put it: the per-user install script,
    // then the locations a system package uses. Checked before the source
    // tree so a packaged install never picks up a stale checkout.
    let candidates = [
        format!("{}/.local/bin/evdi_helper", home),
        "/usr/lib/uscreen/evdi_helper".to_string(),
        "/usr/lib64/uscreen/evdi_helper".to_string(),
        "/usr/libexec/uscreen/evdi_helper".to_string(),
        "/usr/local/lib/uscreen/evdi_helper".to_string(),
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return p;
        }
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
///
/// Presence is published on `tablet_tx` so the capture manager can bring the
/// virtual display up and down along with the tablet.
async fn adb_monitor(
    video_port: u16,
    input_port: u16,
    auto_launch: bool,
    tablet_tx: watch::Sender<bool>,
    token: Option<String>,
    relaunch: std::sync::Arc<tokio::sync::Notify>,
) {
    let mut current: Option<String> = None;
    let mut last_relaunch = std::time::Instant::now() - std::time::Duration::from_secs(60);

    loop {
        let found = adb_device_serial().await;

        match (&current, &found) {
            // Newly attached, or a different tablet than before.
            (None, Some(serial)) => {
                info!(
                    "Tablet connected over {} ({})",
                    transport_of(serial).label(),
                    serial
                );
                announce_transport(serial);
                on_tablet_connected(serial, video_port, input_port, auto_launch, token.as_deref()).await;
                let _ = tablet_tx.send(true);
                current = found;
            }
            (Some(old), Some(serial)) if old != serial => {
                // Usually not a different tablet at all: pulling the cable on a
                // tablet that also has `adb tcpip` running swaps one serial for
                // another on the same device.
                if transport_of(old) != transport_of(serial) {
                    info!(
                        "Tablet moved from {} to {} ({})",
                        transport_of(old).label(),
                        transport_of(serial).label(),
                        serial
                    );
                } else {
                    info!("Different tablet connected ({} → {})", old, serial);
                }
                announce_transport(serial);
                on_tablet_connected(serial, video_port, input_port, auto_launch, token.as_deref()).await;
                let _ = tablet_tx.send(true);
                current = found;
            }
            (Some(old), None) => {
                info!("Tablet disconnected ({})", old);
                let _ = tablet_tx.send(false);
                current = None;
            }
            _ => {}
        }

        // Poll every two seconds, but wake at once if a client turned up
        // without the token: the app was started by hand, and launching it
        // again over adb is how it gets one. Rate-limited so a misbehaving
        // client cannot make us hammer adb.
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(2)) => {}
            _ = relaunch.notified() => {
                if let Some(serial) = current.as_deref() {
                    if last_relaunch.elapsed() >= std::time::Duration::from_secs(5) {
                        last_relaunch = std::time::Instant::now();
                        info!("Delivering the session token to the app");
                        launch_app(serial, token.as_deref()).await;
                    }
                }
            }
        }
    }
}

/// Measured on a quiet network: the median roughly doubles, but the 95th
/// percentile goes from about 28ms to over 150ms and individual frames have
/// been seen at three quarters of a second. Worth saying out loud, because
/// "it works" and "it is pleasant to draw on" are not the same claim.
fn announce_transport(serial: &str) {
    if transport_of(serial) == Transport::Network {
        warn!("Running over Wi-Fi. Expect occasional stutter — the cable is much steadier.");
    }
}

async fn on_tablet_connected(
    serial: &str,
    video_port: u16,
    input_port: u16,
    auto_launch: bool,
    token: Option<&str>,
) {
    match setup_adb_forwarding(serial, video_port, input_port).await {
        Ok(_) => {
            info!("ADB port forwarding set up ({}, {})", video_port, input_port);
            if auto_launch {
                launch_app(serial, token).await;
            }
        }
        Err(e) => warn!("ADB forwarding failed: {}", e),
    }
}

/// Start (or re-front) the app, handing it the session token as an intent
/// extra. The activity is singleTask, so a running app receives it through
/// onNewIntent rather than being restarted.
async fn launch_app(serial: &str, token: Option<&str>) {
    let mut args: Vec<String> = vec![
        "-s".into(), serial.into(), "shell".into(), "am".into(), "start".into(),
        "-n".into(), "com.uscreen/.MainActivity".into(),
    ];
    if let Some(t) = token {
        args.extend(["--es".into(), "token".into(), t.into()]);
    }
    let r = tokio::process::Command::new("adb").args(&args).output().await;
    match r {
        Ok(o) if o.status.success() => info!("UScreen app launched on tablet"),
        _ => warn!("Could not launch the app (is it installed?)"),
    }
}

/// Serial of the first fully-online device, or `None`.
///
/// Deliberately not `adb get-state`: that command fails outright with
/// "more than one device/emulator" as soon as a second device (a phone, an
/// emulator) is attached, which used to turn plug-and-play off with no
/// indication of why.
/// How the tablet is reached. Nothing in the pipeline is tied to either — it
/// speaks to whatever adb is connected to — but the difference is worth a
/// dozen milliseconds, so it is worth naming.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transport {
    Usb,
    Network,
}

impl Transport {
    fn label(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::Network => "Wi-Fi",
        }
    }
}

/// A network device's serial is its `host:port`; a USB serial never contains a
/// colon. That is the whole distinction adb gives us without a second call.
fn transport_of(serial: &str) -> Transport {
    if serial.contains(':') {
        Transport::Network
    } else {
        Transport::Usb
    }
}

/// Pick the tablet to drive, preferring USB.
///
/// Both can be present at once — `adb tcpip` leaves the cable working — and
/// the order adb happens to list them in is not something to hang a latency
/// difference on. USB wins whenever it is there.
async fn adb_device_serial() -> Option<String> {
    let out = tokio::process::Command::new("adb")
        .arg("devices")
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ready: Vec<&str> = text
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            (state == "device").then_some(serial)
        })
        .collect();

    ready
        .iter()
        .find(|s| transport_of(s) == Transport::Usb)
        .or_else(|| ready.first())
        .map(|s| s.to_string())
}

async fn setup_adb_forwarding(serial: &str, video_port: u16, input_port: u16) -> Result<()> {
    for port in [video_port, input_port] {
        let arg = format!("tcp:{}", port);
        let r = tokio::process::Command::new("adb")
            .args(["-s", serial, "reverse", &arg, &arg])
            .output()
            .await?;
        if !r.status.success() {
            anyhow::bail!(
                "adb reverse {} failed: {}",
                arg,
                String::from_utf8_lossy(&r.stderr).trim()
            );
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
