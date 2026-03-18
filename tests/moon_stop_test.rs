use predicates::str::contains;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

fn prepare_moon_home(path: &std::path::Path) {
    fs::create_dir_all(path).expect("mkdir moon home");
    fs::write(path.join(".env"), "\n").expect("write moon env");
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

#[test]
#[cfg(not(windows))]
fn moon_stop_terminates_watcher_daemon_from_json_lock_payload() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    prepare_moon_home(&moon_home);
    let logs_dir = tmp.path().join("moon").join("logs");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");
    let lock_path = logs_dir.join("moon-watch.daemon.lock");

    let mut child = Command::new("sh")
        .arg("-c")
        .arg("while :; do sleep 1; done")
        .arg("watch")
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake daemon");

    fs::write(
        &lock_path,
        format!(
            "{{\"pid\":{},\"started_at_epoch_secs\":1700000000,\"build_uuid\":\"test\",\"moon_home\":\"{}\"}}\n",
            child.id(),
            tmp.path().display()
        ),
    )
    .expect("write json lock payload");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("stop")
        .assert()
        .success()
        .stdout(contains("stopped moon watcher daemon pid="));

    let mut exited = false;
    for _ in 0..40 {
        if child.try_wait().expect("try_wait").is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !exited {
        let _ = child.kill();
    }
    assert!(exited, "fake daemon process did not stop");
    assert!(!lock_path.exists(), "daemon lock should be removed");
}

#[test]
fn moon_stop_is_idempotent_when_lock_is_missing() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    prepare_moon_home(&moon_home);
    let logs_dir = tmp.path().join("moon").join("logs");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("stop")
        .assert()
        .success()
        .stdout(contains("already stopped"));
}

#[test]
#[cfg(not(windows))]
fn moon_stop_terminates_stray_restart_helper_when_lock_is_missing() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon-home");
    prepare_moon_home(&moon_home);
    let logs_dir = tmp.path().join("moon").join("logs");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");

    let mut child = Command::new("perl")
        .arg("-e")
        .arg("sleep 60")
        .arg(format!("MOON_HOME={}", moon_home.display()))
        .arg("moon")
        .arg("restart")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake restart helper");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("stop")
        .assert()
        .success()
        .stdout(contains("stopped moon watcher daemon pid="));

    let mut exited = false;
    for _ in 0..40 {
        if child.try_wait().expect("try_wait").is_some() {
            exited = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !exited {
        let _ = child.kill();
    }
    assert!(exited, "fake restart helper process did not stop");
}

#[test]
#[cfg(not(windows))]
fn moon_stop_skips_ps_fallback_when_pgrep_reports_no_matches() {
    let tmp = tempdir().expect("tempdir");
    let home_dir = tmp.path().join("home");
    let moon_home = tmp.path().join("moon-home");
    let logs_dir = moon_home.join("logs");
    let bin_dir = tmp.path().join("bin");
    let ps_marker = tmp.path().join("ps-called");

    fs::create_dir_all(&home_dir).expect("mkdir home");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    prepare_moon_home(&moon_home);

    write_executable(&bin_dir.join("pgrep"), "#!/bin/sh\nexit 1\n");
    write_executable(
        &bin_dir.join("ps"),
        &format!(
            "#!/bin/sh\necho called > \"{}\"\nexit 99\n",
            ps_marker.display()
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home_dir)
        .env("PATH", &bin_dir)
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("stop")
        .assert()
        .success()
        .stdout(contains("already stopped"));

    assert!(
        !ps_marker.exists(),
        "ps fallback should not run when pgrep reports no matches"
    );
}

#[test]
#[cfg(not(windows))]
fn moon_stop_uses_ps_fallback_when_pgrep_is_unavailable() {
    let tmp = tempdir().expect("tempdir");
    let home_dir = tmp.path().join("home");
    let moon_home = tmp.path().join("moon-home");
    let logs_dir = moon_home.join("logs");
    let bin_dir = tmp.path().join("bin");
    let ps_marker = tmp.path().join("ps-called");

    fs::create_dir_all(&home_dir).expect("mkdir home");
    fs::create_dir_all(&logs_dir).expect("mkdir logs");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");
    prepare_moon_home(&moon_home);

    write_executable(
        &bin_dir.join("ps"),
        &format!(
            "#!/bin/sh\necho called > \"{}\"\nexit 0\n",
            ps_marker.display()
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("HOME", &home_dir)
        .env("PATH", &bin_dir)
        .env("MOON_HOME", &moon_home)
        .env("MOON_LOGS_DIR", &logs_dir)
        .arg("stop")
        .assert()
        .success()
        .stdout(contains("already stopped"));

    assert!(
        ps_marker.exists(),
        "ps fallback should run when pgrep is unavailable"
    );
}
