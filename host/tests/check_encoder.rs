use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn encoder_check_reports_the_probe_failure_without_changing_daemon_state() {
    let root = std::env::temp_dir().join(format!("uscreen-probe-test-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".config/uscreen")).unwrap();
    std::fs::create_dir_all(root.join(".local/share/uscreen")).unwrap();
    let config = root.join(".config/uscreen/config.toml");
    let pid = root.join(".local/share/uscreen/uscreen.pid");
    std::fs::write(&config, "encoder = \"libx264\"\n").unwrap();
    std::fs::write(&pid, std::process::id().to_string()).unwrap();
    let ffmpeg = root.join("ffmpeg");
    std::fs::write(&ffmpeg, "#!/bin/sh\necho '[h264_nvenc @ 0x123] Cannot load libcuda.so.1' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();

    let check = |encoder: &str| Command::new(env!("CARGO_BIN_EXE_uscreen"))
        .args(["check-encoder", "--", encoder])
        .env("HOME", &root)
        .env("PATH", &root)
        .output().unwrap();
    let output = check("h264_nvenc");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("Cannot load libcuda.so.1"),
        "Expected the actual probe cause, got: {stderr}");

    std::fs::write(&ffmpeg, "#!/bin/sh\nexit 0\n").unwrap();
    assert!(check("libx264").status.success());

    std::fs::remove_file(&ffmpeg).unwrap();
    let output = check("h264_nvenc");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not run ffmpeg"));

    std::fs::write(&ffmpeg, "#!/bin/sh\nexec /usr/bin/sleep 30\n").unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = check("h264_nvenc");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no frame within 8 s"));

    assert_eq!(std::fs::read_to_string(&config).unwrap(), "encoder = \"libx264\"\n");
    assert_eq!(std::fs::read_to_string(&pid).unwrap(), std::process::id().to_string());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_probe_uses_the_explicit_device_and_explicit_auto_selection() {
    let root = std::env::temp_dir().join(format!("uscreen-device-test-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".config/uscreen")).unwrap();
    let config = root.join(".config/uscreen/config.toml");
    let original = "encoder = \"h264_vaapi\"\nvaapi_device = \"/disk-only\"\n";
    std::fs::write(&config, original).unwrap();
    let ffmpeg = root.join("ffmpeg");
    std::fs::write(&ffmpeg, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n if [ \"$1\" = -vaapi_device ]; then\n  shift\n  echo \"$1\" >> \"$HOME/used-device\"\n  if [ \"$1\" = /dev/dri/renderD128 ]; then\n   echo 'renderD128 cannot encode' >&2\n   exit 1\n  fi\n  exit 0\n fi\n shift\ndone\nexit 2\n").unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
    let check = |device: &str| Command::new(env!("CARGO_BIN_EXE_uscreen"))
        .args(["check-encoder", "--vaapi-device", device, "--", "h264_vaapi"])
        .env("HOME", &root).env("PATH", &root).output().unwrap();
    let output = check("/dev/dri/renderD128");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("renderD128 cannot encode"));
    assert!(check("/dev/dri/renderD129").status.success());
    let _ = check("");
    let devices = std::fs::read_to_string(root.join("used-device")).unwrap();
    assert!(!devices.contains("/disk-only"), "Explicit auto selection must ignore the disk device");
    assert!(devices.lines().all(|device| device.starts_with("/dev/dri/renderD")));
    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_slow_probe_is_not_reported_as_a_daemon() {
    let root = std::env::temp_dir().join(format!("uscreen-status-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let ffmpeg = root.join("ffmpeg");
    std::fs::write(&ffmpeg, "#!/bin/sh\necho ready > \"$HOME/probe-ready\"\nexec /usr/bin/sleep 2\n").unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut probe = Command::new(env!("CARGO_BIN_EXE_uscreen"))
        .args(["check-encoder", "libx264"])
        .env("HOME", &root).env("PATH", &root).spawn().unwrap();
    let probe_pid = probe.id().to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !root.join("probe-ready").exists() {
        assert!(std::time::Instant::now() < deadline, "Probe did not reach ffmpeg");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let status = Command::new(env!("CARGO_BIN_EXE_uscreen"))
        .arg("status").env("HOME", &root).output().unwrap();
    let output = String::from_utf8_lossy(&status.stdout);
    let reported = output.split(|c: char| !c.is_ascii_digit()).any(|token| token == probe_pid);
    assert!(probe.wait().unwrap().success());
    std::fs::remove_dir_all(root).unwrap();
    assert!(!reported, "A check-only process must not be listed as a daemon: {output}");
}
