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
const MAX_BACKLOG: usize = 4;

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

        let addr = format!("0.0.0.0:{}", self.config.video_port);
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
                Self::write_packet(&mut socket, PACKET_TYPE_FRAME, &packet.data).await?;
            }

            // Flush once per batch for lowest latency without extra syscalls
            socket.flush().await?;
        }

        Ok(())
    }

    async fn write_packet(socket: &mut TcpStream, packet_type: u8, payload: &[u8]) -> Result<()> {
        let packet_len = payload.len() + 1;
        let len_buf = (packet_len as u32).to_be_bytes();
        socket.write_all(&len_buf).await?;
        socket.write_all(&[packet_type]).await?;
        socket.write_all(payload).await?;
        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
