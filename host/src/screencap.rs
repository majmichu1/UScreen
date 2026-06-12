use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Instant;
use tracing::{error, info, warn};
use zbus::zvariant::{Fd, OwnedValue, Value};
use zbus::Connection;

pub struct ScreenCapture {
    screen_name: String,
}

impl ScreenCapture {
    pub fn new(screen_name: &str) -> Self {
        Self {
            screen_name: screen_name.to_string(),
        }
    }

    pub async fn run(&self, fifo_path: &str) -> Result<()> {
        info!("ScreenCapture: connecting to D-Bus...");
        let conn = Connection::session().await?;
        info!("ScreenCapture: connected, opening FIFO (will block until ffmpeg reads)...");

        let fifo_path = fifo_path.to_string();
        let fifo = tokio::task::spawn_blocking(move || {
            std::fs::File::create(&fifo_path)
                .context("Failed to open FIFO for writing")
        })
        .await
        .context("FIFO open task failed")??;

        info!("ScreenCapture: FIFO opened, starting capture loop for '{}'", self.screen_name);

        let mut frame_count: u64 = 0;
        let mut last_stats = Instant::now();
        let mut stats_frames = 0u64;

        loop {
            let frame_start = Instant::now();

            // Create a pipe for passing frame data from D-Bus
            let mut fds = [0i32; 2];
            let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
            if ret < 0 {
                anyhow::bail!("pipe failed: {}", std::io::Error::last_os_error());
            }
            let reader = unsafe { OwnedFd::from_raw_fd(fds[0]) };
            let writer = unsafe { OwnedFd::from_raw_fd(fds[1]) };
            let writer_fd = Fd::from(writer);

            let mut options = HashMap::new();
            options.insert("format", Value::from("BGRA"));
            options.insert("cursor", Value::from(false));

            let result: HashMap<String, OwnedValue> = conn
                .call_method(
                    Some("org.kde.KWin.ScreenShot2"),
                    "/org/kde/KWin/ScreenShot2",
                    Some("org.kde.KWin.ScreenShot2"),
                    "CaptureScreen",
                    &(self.screen_name.as_str(), options, &writer_fd),
                )
                .await?
                .body()
                .deserialize()?;

            // Parse metadata
            let get_u32 = |key: &str| -> u32 {
                result
                    .get(key)
                    .and_then(|v| v.downcast_ref::<u32>().ok())
                    .unwrap_or(0)
            };

            let width = get_u32("width");
            let height = get_u32("height");
            let stride = get_u32("stride");

            if width == 0 || height == 0 || stride == 0 {
                warn!("Bad capture: {}x{} stride={}", width, height, stride);
                tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                continue;
            }

            // Read frame from pipe
            let frame_size = stride as usize * height as usize;
            let mut buffer = vec![0u8; frame_size];
            let fd = reader.as_raw_fd();
            let mut offset = 0;
            while offset < frame_size {
                let n = unsafe {
                    libc::read(
                        fd,
                        buffer.as_mut_ptr().add(offset) as *mut libc::c_void,
                        frame_size - offset,
                    )
                };
                if n < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    anyhow::bail!("pipe read: {}", err);
                }
                if n == 0 {
                    break;
                }
                offset += n as usize;
            }

            // Write to FIFO
            if let Err(e) = (&fifo).write_all(&buffer) {
                error!("FIFO write: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }

            frame_count += 1;
            stats_frames += 1;

            // Frame pacing ~60fps
            let elapsed = frame_start.elapsed();
            if elapsed < std::time::Duration::from_millis(16) {
                tokio::time::sleep(std::time::Duration::from_millis(16) - elapsed).await;
            }

            // Stats every 5s
            if last_stats.elapsed().as_secs() >= 5 {
                let fps = stats_frames as f64 / last_stats.elapsed().as_secs_f64();
                info!("ScreenCapture: {} fps, total: {}", fps as u32, frame_count);
                stats_frames = 0;
                last_stats = Instant::now();
            }
        }
    }
}
