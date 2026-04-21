use crate::moon::paths::MoonPaths;
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const DAEMON_LOCK_FILE: &str = "moon-watch.daemon.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonLockPayload {
    pub pid: u32,
    #[serde(default, alias = "start_time")]
    pub started_at_epoch_secs: u64,
    #[serde(default)]
    pub build_uuid: String,
    #[serde(default)]
    pub moon_home: String,
}

#[derive(Debug)]
pub struct DaemonLockGuard {
    path: PathBuf,
    file: Option<File>,
}

pub fn daemon_lock_path(paths: &MoonPaths) -> PathBuf {
    paths.logs_dir.join(DAEMON_LOCK_FILE)
}

pub fn parse_daemon_lock_payload(raw: &str) -> Option<DaemonLockPayload> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    serde_json::from_str::<DaemonLockPayload>(trimmed).ok()
}

pub fn read_daemon_lock_payload(paths: &MoonPaths) -> Result<Option<DaemonLockPayload>> {
    let lock_path = daemon_lock_path(paths);
    if !lock_path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&lock_path)
        .with_context(|| format!("failed to read daemon lock {}", lock_path.display()))?;
    Ok(parse_daemon_lock_payload(&raw))
}

fn write_daemon_lock_payload(
    file: &mut File,
    lock_path: &Path,
    payload: &DaemonLockPayload,
) -> Result<()> {
    file.set_len(0)
        .with_context(|| format!("failed to reset daemon lock {}", lock_path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("failed to seek daemon lock {}", lock_path.display()))?;
    file.write_all(format!("{}\n", serde_json::to_string(payload)?).as_bytes())
        .with_context(|| format!("failed to write daemon lock {}", lock_path.display()))?;
    file.sync_data()
        .with_context(|| format!("failed to sync daemon lock {}", lock_path.display()))?;
    Ok(())
}

pub fn acquire_daemon_lock(
    paths: &MoonPaths,
    payload: &DaemonLockPayload,
) -> Result<DaemonLockGuard> {
    let lock_path = daemon_lock_path(paths);
    if let Some(parent) = lock_path.parent() {
        crate::moon::fs_security::ensure_private_dir(parent)?;
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open daemon lock {}", lock_path.display()))?;

    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            if let Ok(Some(existing)) = read_daemon_lock_payload(paths) {
                anyhow::bail!(
                    "moon watcher daemon already running pid={} started_at_epoch_secs={} moon_home={}",
                    existing.pid,
                    existing.started_at_epoch_secs,
                    if existing.moon_home.trim().is_empty() {
                        "<unknown>"
                    } else {
                        existing.moon_home.trim()
                    }
                );
            }
            anyhow::bail!(
                "moon watcher daemon already running (lock held at {})",
                lock_path.display()
            );
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to lock daemon file {}", lock_path.display()));
        }
    }

    write_daemon_lock_payload(&mut file, &lock_path, payload)?;
    Ok(DaemonLockGuard {
        path: lock_path,
        file: Some(file),
    })
}

impl DaemonLockGuard {
    fn unlock(&mut self) -> Result<()> {
        let Some(file) = self.file.take() else {
            return Ok(());
        };

        file.unlock()
            .with_context(|| format!("failed to unlock daemon lock {}", self.path.display()))
    }

    pub fn release(mut self) -> Result<()> {
        self.unlock()?;
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to remove daemon lock {}", self.path.display())
                });
            }
        }
        Ok(())
    }
}

impl Drop for DaemonLockGuard {
    fn drop(&mut self) {
        let _ = self.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonLockPayload, acquire_daemon_lock, daemon_lock_path, parse_daemon_lock_payload,
    };
    use crate::moon::paths::MoonPaths;
    use std::fs;
    use std::path::Path;
    #[cfg(not(windows))]
    use std::process::{Command, Stdio};
    #[cfg(not(windows))]
    use std::thread;
    #[cfg(not(windows))]
    use std::time::Duration;
    use tempfile::tempdir;

    fn test_paths(moon_home: &Path) -> MoonPaths {
        let home = moon_home.parent().unwrap_or(moon_home).to_path_buf();
        MoonPaths {
            moon_home: moon_home.to_path_buf(),
            raw_dir: moon_home.join("raw"),
            mds_dir: moon_home.join("mds"),
            mlib_dir: moon_home.join("mlib"),
            cleanse_dir: moon_home.join("cleanse"),
            memory_dir: moon_home.join("memory"),
            memory_file: moon_home.join("MEMORY.md"),
            logs_dir: moon_home.join("logs"),
            context_engine_dir: moon_home.join("mce"),
            context_packet_dir: moon_home.join("mcp"),
            openclaw_sessions_dir: home.join(".openclaw/agents/main/sessions"),
            qmd_bin: home.join(".bun/bin/qmd"),
            qmd_db: moon_home.join("qmd/index.sqlite"),
            qmd_config_dir: moon_home.join("qmd/config"),
            moon_home_is_explicit: true,
        }
    }

    #[test]
    fn parses_json_payload() {
        let raw = r#"{"pid":42,"started_at_epoch_secs":1700000000,"build_uuid":"abc","moon_home":"/tmp/moon"}"#;
        let payload = parse_daemon_lock_payload(raw).expect("payload");
        assert_eq!(payload.pid, 42);
        assert_eq!(payload.build_uuid, "abc");
    }

    #[test]
    fn graceful_release_removes_lock_file() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join(".moon");
        let paths = test_paths(&moon_home);
        let guard = acquire_daemon_lock(
            &paths,
            &DaemonLockPayload {
                pid: 42,
                started_at_epoch_secs: 1700000000,
                build_uuid: "test".to_string(),
                moon_home: moon_home.display().to_string(),
            },
        )
        .expect("acquire daemon lock");

        let lock_path = daemon_lock_path(&paths);
        assert!(lock_path.exists(), "lock file should exist while held");

        guard.release().expect("release daemon lock");
        assert!(
            !lock_path.exists(),
            "lock file should be removed after graceful shutdown"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn acquire_reports_existing_locked_owner() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join(".moon");
        let paths = test_paths(&moon_home);
        fs::create_dir_all(&paths.logs_dir).expect("mkdir logs");

        let helper = tmp.path().join("hold_lock.pl");
        fs::write(
            &helper,
            r#"use Fcntl qw(:flock SEEK_SET);
my ($path, $moon_home) = @ARGV;
open my $fh, "+>>", $path or die $!;
flock($fh, LOCK_EX) or die $!;
seek($fh, 0, SEEK_SET) or die $!;
truncate($fh, 0) or die $!;
syswrite($fh, "{\"pid\":4242,\"started_at_epoch_secs\":1700000000,\"build_uuid\":\"helper\",\"moon_home\":\"$moon_home\"}\n") or die $!;
select(undef, undef, undef, 0.2);
sleep 30;
"#,
        )
        .expect("write helper");

        let lock_path = daemon_lock_path(&paths);
        let mut child = Command::new("perl")
            .arg(&helper)
            .arg(&lock_path)
            .arg(moon_home.display().to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn helper");

        let mut ready = false;
        for _ in 0..20 {
            if let Ok(Some(payload)) = super::read_daemon_lock_payload(&paths)
                && payload.pid == 4242
            {
                ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "helper never published the daemon lock payload");

        let err = acquire_daemon_lock(
            &paths,
            &DaemonLockPayload {
                pid: 77,
                started_at_epoch_secs: 1700000001,
                build_uuid: "test".to_string(),
                moon_home: moon_home.display().to_string(),
            },
        )
        .expect_err("second daemon acquisition should fail");

        let err_text = format!("{err:#}");
        assert!(
            err_text.contains("already running"),
            "error should explain the daemon is already running: {err_text}"
        );
        assert!(
            err_text.contains("pid=4242"),
            "error should include the existing daemon pid: {err_text}"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
