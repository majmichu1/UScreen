//! Which video encoders actually work on this machine.
//!
//! `ffmpeg -encoders` lists what ffmpeg was *built* with, which is every
//! hardware encoder under the sun on a distribution build — the AMD laptop
//! in issue #15 had `h264_nvenc` "available" by that test and failed with
//! "Cannot load libcuda.so.1" the moment it was used. The only honest check
//! is to encode a frame. One tiny frame takes well under a second and only
//! happens when the configured encoder is in doubt.

use tracing::{info, warn};

/// In order of preference. NVENC and VAAPI are the two hardware paths this
/// project tunes for; libx264 works everywhere and is what a laptop without
/// either falls back to.
pub const CANDIDATES: [&str; 3] = ["h264_nvenc", "h264_vaapi", "libx264"];

/// Extra ffmpeg arguments an encoder needs before it will accept a frame.
fn setup_args(encoder: &str) -> Vec<&'static str> {
    match encoder {
        "h264_vaapi" | "hevc_vaapi" => vec![
            "-vaapi_device",
            "/dev/dri/renderD128",
            "-vf",
            "format=nv12,hwupload",
        ],
        _ => vec![],
    }
}

/// Encode one 256x144 frame with `encoder`, bounded to a few seconds.
pub async fn works(encoder: &str) -> bool {
    let mut args: Vec<&str> = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "color=size=256x144:rate=1",
        // NVENC refuses anything smaller than about 145x49, and lavfi's
        // default is RGB, which no hardware encoder takes.
        "-pix_fmt",
        "nv12",
        "-frames:v",
        "1",
    ];
    args.extend(setup_args(encoder));
    args.extend(["-c:v", encoder, "-f", "null", "-"]);
    let run = tokio::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(8), run).await {
        Ok(Ok(out)) => out.status.success(),
        Ok(Err(_)) => false,
        Err(_) => false,
    }
}

/// The first encoder in `CANDIDATES` that works, if any.
pub async fn first_working() -> Option<&'static str> {
    for e in CANDIDATES {
        if works(e).await {
            return Some(e);
        }
    }
    None
}

/// `configured` if it works, otherwise the first candidate that does — with
/// a log line saying why the configuration was overridden. `None` only when
/// nothing encodes at all, in which case the caller should say so and stop.
pub async fn resolve(configured: &str) -> Option<String> {
    // The old gstreamer-style alias.
    let configured = if configured == "vaapih264enc" { "h264_vaapi" } else { configured };
    if works(configured).await {
        return Some(configured.to_string());
    }
    match first_working().await {
        Some(e) => {
            warn!(
                "Encoder {} does not work on this machine — using {} instead. \
                 (Set `encoder` in the config or the GUI to keep a different choice.)",
                configured, e
            );
            Some(e.to_string())
        }
        None => {
            info!("No H.264 encoder works here: tried {}", CANDIDATES.join(", "));
            None
        }
    }
}
