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
/// The error is ffmpeg's own last line, which is what a bug report needs —
/// "cannot encode" alone does not say whether the driver, the device node
/// or the ffmpeg build is at fault.
pub async fn probe(encoder: &str) -> Result<(), String> {
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
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(8), run).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            // The first line is the cause ("Cannot load libcuda.so.1");
            // the last is ffmpeg's generic "Nothing was written" epilogue.
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("ffmpeg failed without a message");
            // Drop the "[h264_amf @ 0x55bd…] " context prefixes.
            let mut msg = first;
            while let Some(rest) = msg.strip_prefix('[').and_then(|r| r.split_once("] ")) {
                msg = rest.1;
            }
            Err(msg.to_string())
        }
        Ok(Err(e)) => Err(format!("could not run ffmpeg: {}", e)),
        Err(_) => Err("no frame within 8 s".to_string()),
    }
}

pub async fn works(encoder: &str) -> bool {
    probe(encoder).await.is_ok()
}

/// `configured` if it works, otherwise the first candidate that does — with
/// a log line saying why the configuration was overridden. `None` only when
/// nothing encodes at all, in which case the caller should say so and stop.
pub async fn resolve(configured: &str) -> Option<String> {
    // The old gstreamer-style alias.
    let configured = if configured == "vaapih264enc" { "h264_vaapi" } else { configured };
    let first = match probe(configured).await {
        Ok(()) => return Some(configured.to_string()),
        Err(e) => e,
    };
    // Once more before overriding what the user chose: at login a hybrid
    // laptop's discrete GPU may still be waking up, or its driver still
    // loading, and replacing the encoder is saved to the config.
    info!("Encoder {} failed a test frame ({}), trying once more", configured, first);
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let reason = match probe(configured).await {
        Ok(()) => return Some(configured.to_string()),
        Err(e) => e,
    };
    let mut fallback = None;
    for e in CANDIDATES.into_iter().filter(|e| *e != configured) {
        if works(e).await {
            fallback = Some(e);
            break;
        }
    }
    match fallback {
        Some(e) => {
            warn!(
                "Encoder {} does not work on this machine ({}) — using {} instead. \
                 (Set `encoder` in the config or the GUI to keep a different choice.)",
                configured, reason, e
            );
            Some(e.to_string())
        }
        None => {
            info!("No H.264 encoder works here: tried {}", CANDIDATES.join(", "));
            None
        }
    }
}
