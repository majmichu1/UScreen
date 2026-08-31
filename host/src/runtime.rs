//! Per-user runtime state: the capture FIFO and the session token.
//!
//! Both used to live in /tmp, world-readable. On a multi-user machine that
//! meant any local account could open the FIFO and read the raw frames — a
//! live copy of the screen — or write into it and corrupt the stream. The
//! runtime directory is per-user and mode 0700, so neither is possible now.

use anyhow::{Context, Result};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR/uscreen`, created 0700. Falls back to `~/.cache/uscreen`
/// when the session has no runtime dir (a plain SSH login, for instance).
pub fn runtime_dir() -> PathBuf {
    // /run/user/<uid> next: the daemon under systemd and a `doctor` run from
    // an environment-scrubbed shell (sudo, cron) must agree on the path, or
    // the orphan check looks for a FIFO that is somewhere else.
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| {
            let p = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
            p.is_dir().then_some(p)
        })
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".cache")
        });
    let dir = base.join("uscreen");
    if !dir.is_dir() {
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir);
    }
    dir
}

pub fn fifo_path() -> PathBuf {
    fifo_path_for(0)
}

/// One FIFO per virtual display; the first keeps the old name.
pub fn fifo_path_for(instance: u32) -> PathBuf {
    if instance == 0 {
        runtime_dir().join("capture.fifo")
    } else {
        runtime_dir().join(format!("capture-{}.fifo", instance))
    }
}

fn token_path() -> PathBuf {
    runtime_dir().join("token")
}

/// 64 hex characters from the kernel's RNG. Generated once per daemon run and
/// written to the runtime directory (0600) for anything else of ours that
/// needs it; it is never sent anywhere except to the tablet, over adb.
pub fn new_session_token() -> Result<String> {
    use std::io::Read;
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("open /dev/urandom")?
        .read_exact(&mut raw)
        .context("read /dev/urandom")?;
    let token: String = raw.iter().map(|b| format!("{:02x}", b)).collect();

    let path = token_path();
    let _ = std::fs::remove_file(&path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("write {}", path.display()))?;
    use std::io::Write;
    f.write_all(token.as_bytes())?;
    Ok(token)
}

/// Compare a presented token with the expected one. Constant-time over the
/// expected length, so timing does not leak how many leading characters were
/// right — cheap insurance on a loopback socket.
pub fn token_matches(expected: &str, presented: &str) -> bool {
    let a = expected.as_bytes();
    let b = presented.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_is_exact() {
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc123", "abc12"));
        assert!(!token_matches("abc123", ""));
    }

    #[test]
    fn runtime_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = runtime_dir();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        // Either we created it 0700, or it is the user's own cache dir.
        assert_eq!(mode & 0o077, 0, "runtime dir {} is group/world accessible", dir.display());
    }
}
