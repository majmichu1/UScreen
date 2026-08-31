//! "A newer release exists" — nothing more.
//!
//! This deliberately does not download or install anything. On Linux the
//! package manager is the update mechanism, and a daemon that overwrites its
//! own binary behind the package manager's back is how systems end up in
//! states nobody can explain. So the daemon only asks GitHub what the latest
//! tag is, and the tray icon and `uscreen doctor` say so if it is newer.
//!
//! Uses curl rather than an HTTP client crate: one HTTPS GET a day is not
//! worth a dependency tree, and curl is on every system this runs on.

use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info};

const RELEASES_API: &str = "https://api.github.com/repos/majmichu1/UScreen/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/majmichu1/UScreen/releases/latest";

/// Delay before the first check, so startup is not spent waiting on the
/// network, and the interval between checks after that.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Version string of a newer release, if one exists.
pub type Available = Option<String>;

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse "1.2.3" into comparable parts. Anything unparseable sorts lowest, so
/// a malformed tag can never look like an upgrade.
fn parse(v: &str) -> (u32, u32, u32) {
    let v = v.trim().trim_start_matches('v');
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse(candidate) > parse(current)
}

/// Extract `tag_name` from the release JSON without pulling in a full parse
/// of everything else GitHub sends back.
fn tag_from_json(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("tag_name")?.as_str().map(|s| s.to_string())
}

pub async fn latest_release_tag() -> Option<String> {
    let out = tokio::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "4",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            &format!("User-Agent: uscreen/{}", current_version()),
            RELEASES_API,
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        debug!("update check: curl exited {}", out.status);
        return None;
    }
    tag_from_json(&String::from_utf8_lossy(&out.stdout))
}

/// Runs for the life of the daemon, publishing the newer version (if any) on
/// `tx`. Failures are silent at info level and below: no network is not an
/// error condition for a second-screen daemon.
pub async fn run(tx: watch::Sender<Available>) {
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        if let Some(tag) = latest_release_tag().await {
            let latest = tag.trim_start_matches('v').to_string();
            if is_newer(&latest, current_version()) {
                info!(
                    "A newer release is available: {} (running {}). {}",
                    latest,
                    current_version(),
                    RELEASES_PAGE
                );
                let _ = tx.send(Some(latest));
            } else {
                debug!("update check: {} is current", current_version());
                let _ = tx.send(None);
            }
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("1.1.0", "1.0.2"));
        assert!(is_newer("v1.1.0", "1.0.2"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.2", "1.0.2"));
        assert!(!is_newer("1.0.1", "1.0.2"));
        assert!(!is_newer("garbage", "1.0.2"));
    }

    #[test]
    fn tag_is_read_from_release_json() {
        let body = r#"{"url":"x","tag_name":"v1.4.0","name":"v1.4.0","assets":[]}"#;
        assert_eq!(tag_from_json(body).as_deref(), Some("v1.4.0"));
        assert_eq!(tag_from_json("not json"), None);
    }
}
