use crate::capture::VideoPacket;
use anyhow::Result;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

const PACKET_TYPE_CONFIG: u8 = 0;
const PACKET_TYPE_FRAME: u8 = 1;

/// If more than this many frames are queued for a client, skip ahead to the
/// most recent IDR instead of letting latency accumulate.
const MAX_BACKLOG: usize = 2;

/// Kernel send-buffer cap, in bytes. Roughly a couple of frames' worth at the
/// rates this streams at — enough to absorb scheduling jitter, too small to
/// hide a genuinely slow link.
const SEND_BUFFER_BYTES: libc::c_int = 128 * 1024;

pub struct StreamConfig {
    pub video_port: u16,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self { video_port: 8890 }
    }
}

pub struct StreamServer {
    config: StreamConfig,
    running: Arc<AtomicBool>,
    codec_config: Arc<Mutex<Option<Bytes>>>,
}

impl StreamServer {
    pub fn new(config: StreamConfig, codec_config: Arc<Mutex<Option<Bytes>>>) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            codec_config,
        }
    }

    pub async fn run(&self, video_rx: broadcast::Receiver<VideoPacket>) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);

        // Loopback only. The tablet reaches us through `adb reverse`, where the
        // adb server on this machine opens the connection locally — binding all
        // interfaces would put an unauthenticated live view of the screen on
        // the LAN for anyone who cares to connect.
        let addr = format!("127.0.0.1:{}", self.config.video_port);
        let listener = TcpListener::bind(&addr).await?;

        info!("Stream server on tcp://{}", addr);

        let running = self.running.clone();

        loop {
            let accept = tokio::select! {
                res = listener.accept() => res,
                _ = async {
                    while running.load(Ordering::SeqCst) {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                } => break,
            };

            let (socket, peer) = match accept {
                Ok(s) => s,
                Err(e) => {
                    error!("Accept failed: {}", e);
                    continue;
                }
            };

            info!("Client connected: {}", peer);
            let rx = video_rx.resubscribe();
            let cc = self.codec_config.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(socket, rx, cc).await {
                    warn!("Client {} disconnected: {}", peer, e);
                }
                info!("Client {} session ended", peer);
            });
        }

        Ok(())
    }

    async fn handle_client(
        mut socket: TcpStream,
        mut rx: broadcast::Receiver<VideoPacket>,
        codec_config: Arc<Mutex<Option<Bytes>>>,
    ) -> Result<()> {
        // Disable Nagle's algorithm for lower latency
        socket.set_nodelay(true)?;

        // Cap the kernel send buffer. Linux auto-tunes this into the megabytes,
        // which on a link slower than the encoder means frames sit invisibly in
        // the socket instead of surfacing as backpressure — the skip-ahead
        // logic below never sees them, and the delay lands on screen instead.
        // A small buffer makes a slow link show up immediately as a blocked
        // write, which is exactly what the backlog handling needs to react to.
        Self::set_send_buffer(&socket, SEND_BUFFER_BYTES);

        let mut last_sent_config: Option<Bytes> = None;

        // Send cached codec config (SPS/PPS) so MediaCodec can configure.
        // If not yet available, wait briefly for it.
        let mut retries = 0;
        loop {
            let codec_data: Option<Bytes> = codec_config.lock().ok().and_then(|g| g.clone());
            if let Some(config) = codec_data {
                info!("Sending codec config to client ({} bytes)", config.len());
                Self::write_packet(&mut socket, PACKET_TYPE_CONFIG, &config).await?;
                socket.flush().await?;
                last_sent_config = Some(config);
                break;
            }
            retries += 1;
            if retries > 50 {
                // 5 seconds
                warn!("Codec config not available after 5s, starting stream without it");
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        // New clients can only start decoding at an IDR
        let mut wait_for_idr = true;
        let mut dropped: u64 = 0;

        loop {
            let first = match rx.recv().await {
                Ok(d) => d,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Client lagged {} frames, resuming at next IDR", n);
                    wait_for_idr = true;
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            // Drain whatever else is already queued so we can see how far
            // behind this client is.
            let mut batch = vec![first];
            loop {
                match rx.try_recv() {
                    Ok(p) => batch.push(p),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        warn!("Client lagged {} frames, resuming at next IDR", n);
                        wait_for_idr = true;
                        batch.clear();
                    }
                    Err(_) => break,
                }
            }

            // Too far behind: jump to the freshest IDR if one is queued.
            // Frames before an IDR are never needed to decode what follows it.
            if batch.len() > MAX_BACKLOG {
                if let Some(pos) = batch.iter().rposition(|p| p.is_idr) {
                    dropped += pos as u64;
                    batch.drain(..pos);
                }
            }

            let current_config = codec_config.lock().ok().and_then(|g| g.clone());
            if let Some(config) = current_config {
                if last_sent_config.as_ref() != Some(&config) {
                    info!(
                        "Sending refreshed codec config to client ({} bytes)",
                        config.len()
                    );
                    Self::write_packet(&mut socket, PACKET_TYPE_CONFIG, &config).await?;
                    last_sent_config = Some(config);
                    wait_for_idr = true;
                }
            }

            for packet in batch {
                if wait_for_idr {
                    if !packet.is_idr {
                        dropped += 1;
                        continue;
                    }
                    if dropped > 0 {
                        info!("Resumed at IDR after dropping {} frames", dropped);
                        dropped = 0;
                    }
                    wait_for_idr = false;
                }
                Self::write_frame(&mut socket, packet.seq, &packet.data).await?;
            }

            // Flush once per batch for lowest latency without extra syscalls
            socket.flush().await?;
        }

        Ok(())
    }

    /// Best-effort: a kernel that refuses the hint is not a reason to fail the
    /// connection, it just means latency behaves as it did before.
    fn set_send_buffer(socket: &TcpStream, bytes: libc::c_int) {
        use std::os::fd::AsRawFd;
        let fd = socket.as_raw_fd();
        let rc = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &bytes as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            warn!(
                "Could not set SO_SNDBUF: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    async fn write_packet(socket: &mut TcpStream, packet_type: u8, payload: &[u8]) -> Result<()> {
        let packet_len = payload.len() + 1;
        let len_buf = (packet_len as u32).to_be_bytes();
        socket.write_all(&len_buf).await?;
        socket.write_all(&[packet_type]).await?;
        socket.write_all(payload).await?;
        Ok(())
    }

    /// Frame packets carry a 4-byte big-endian sequence number after the type
    /// byte. The tablet hands it to the decoder as the presentation timestamp
    /// and echoes it back once the frame is on screen, which is what makes
    /// end-to-end latency measurable on a single clock.
    async fn write_frame(socket: &mut TcpStream, seq: u32, payload: &[u8]) -> Result<()> {
        let packet_len = payload.len() + 1 + 4;
        socket.write_all(&(packet_len as u32).to_be_bytes()).await?;
        socket.write_all(&[PACKET_TYPE_FRAME]).await?;
        socket.write_all(&seq.to_be_bytes()).await?;
        socket.write_all(payload).await?;
        Ok(())
    }

    /// Counterpart to `run`; shutdown currently goes through task
    /// cancellation instead, but leaving this makes the lifecycle explicit.
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
