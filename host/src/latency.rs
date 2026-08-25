//! End-to-end latency measurement.
//!
//! Every access unit leaving the encoder gets a sequence number and a host
//! timestamp. The tablet echoes the sequence number back over the existing
//! input WebSocket once the frame has actually been rendered, so the round trip
//! is measured entirely on the host's clock — no clock synchronisation between
//! the two machines, which is what makes naive "timestamp in the stream"
//! approaches useless.
//!
//! What this measures is `encoder output → visible on the tablet`: transport
//! queueing, decode and render. The helper reports its own capture→FIFO delay
//! separately on stderr; the two together account for the whole pipeline.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::info;

/// How many in-flight frames to remember. At 60 fps this is ~4s of history,
/// far more than any sane round trip, and bounded so a tablet that stops
/// reporting cannot grow this without limit.
const MAX_TRACKED: usize = 256;

/// Samples kept for the percentile report. One report covers ~5s.
const MAX_SAMPLES: usize = 1024;

#[derive(Default)]
struct Inner {
    /// (seq, time the access unit left the encoder), oldest first.
    sent: VecDeque<(u32, Instant)>,
    /// Round-trip latencies in microseconds, for the current report window.
    samples: Vec<u32>,
    /// Of that round trip, the part the tablet spent decoding and rendering.
    /// The remainder is what the transport costs — which is what decides
    /// whether a different transport is worth building.
    decode_samples: Vec<u32>,
    /// Frames that were sent but whose acknowledgement never arrived before
    /// they aged out — a direct sign of frames being dropped downstream.
    lost: u64,
    last_report: Option<Instant>,
}

#[derive(Clone, Default)]
pub struct LatencyTracker {
    inner: Arc<Mutex<Inner>>,
}

impl LatencyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// An access unit just left the encoder.
    pub fn on_encoded(&self, seq: u32) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.last_report.is_none() {
            g.last_report = Some(Instant::now());
        }
        g.sent.push_back((seq, Instant::now()));
        while g.sent.len() > MAX_TRACKED {
            g.sent.pop_front();
            g.lost += 1;
        }
    }

    /// The tablet reports that this frame is on screen.
    pub fn on_rendered(&self, seq: u32, decode_us: i64) {
        let Ok(mut g) = self.inner.lock() else { return };
        // Everything queued before this frame is now known to be behind it;
        // drop it so the deque tracks only genuinely in-flight frames.
        let Some(pos) = g.sent.iter().position(|(s, _)| *s == seq) else {
            return;
        };
        let (_, at) = g.sent[pos];
        let micros = at.elapsed().as_micros().min(u32::MAX as u128) as u32;
        g.sent.drain(..=pos);
        if g.samples.len() < MAX_SAMPLES {
            g.samples.push(micros);
            if decode_us > 0 {
                g.decode_samples
                    .push((decode_us as u128).min(u32::MAX as u128) as u32);
            }
        }
    }

    /// Log percentiles if the report window has elapsed. Cheap to call often.
    pub fn maybe_report(&self) {
        let Ok(mut g) = self.inner.lock() else { return };
        let Some(last) = g.last_report else { return };
        if last.elapsed().as_secs() < 5 {
            return;
        }
        g.last_report = Some(Instant::now());

        if g.samples.is_empty() {
            // Silence here means the tablet never acknowledged anything, which
            // is itself worth saying out loud.
            if g.lost > 0 {
                info!(
                    "Latency: no frames acknowledged by the tablet ({} aged out)",
                    g.lost
                );
                g.lost = 0;
            }
            return;
        }

        let mut s = std::mem::take(&mut g.samples);
        let mut d = std::mem::take(&mut g.decode_samples);
        let lost = std::mem::take(&mut g.lost);
        let inflight = g.sent.len();
        drop(g);

        s.sort_unstable();
        let pct = |v: &[u32], p: f64| -> f64 {
            let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
            v[idx] as f64 / 1000.0
        };
        let total_p50 = pct(&s, 0.50);
        info!(
            "Latency encode→display: p50 {:.1}ms  p95 {:.1}ms  max {:.1}ms  ({} samples, {} in flight, {} aged out)",
            total_p50,
            pct(&s, 0.95),
            pct(&s, 1.0),
            s.len(),
            inflight,
            lost
        );

        // The split is what tells us where to spend effort: a large decode
        // share means a different transport would buy nothing.
        if !d.is_empty() {
            d.sort_unstable();
            let decode_p50 = pct(&d, 0.50);
            info!(
                "  of which tablet decode+render p50 {:.1}ms  p95 {:.1}ms  → wire ~{:.1}ms",
                decode_p50,
                pct(&d, 0.95),
                (total_p50 - decode_p50).max(0.0)
            );
        }
    }
}
