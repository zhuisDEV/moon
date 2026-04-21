use crate::moon::paths::MoonPaths;
use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;

#[derive(Debug, Clone, Copy)]
pub enum PrivatePathKind {
    File,
    Directory,
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    harden_private_dir_if_exists(path)
}

pub fn harden_private_dir_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .permissions();
        perms.set_mode(PRIVATE_DIR_MODE);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }

    Ok(())
}

pub fn harden_private_file_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .permissions();
        perms.set_mode(PRIVATE_FILE_MODE);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }

    Ok(())
}

pub fn ensure_private_file_with_contents_if_missing(path: &Path, contents: &[u8]) -> Result<bool> {
    if path.exists() {
        harden_private_file_if_exists(path)?;
        return Ok(false);
    }

    write_private_file(path, contents)?;
    Ok(true)
}

pub fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve parent for {}", path.display()))?;
    ensure_private_dir(parent)?;

    let tmp_path = path.with_extension(match path.extension().and_then(|v| v.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{ext}.tmp"),
        _ => "tmp".to_string(),
    });

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(PRIVATE_FILE_MODE);
    }
    let mut file = options
        .open(&tmp_path)
        .with_context(|| format!("failed to open {}", tmp_path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp_path.display()))?;
    harden_private_file_if_exists(&tmp_path)?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to move {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    harden_private_file_if_exists(path)?;
    Ok(())
}

pub fn open_private_append(path: &Path) -> Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve parent for {}", path.display()))?;
    ensure_private_dir(parent)?;

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        options.mode(PRIVATE_FILE_MODE);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    harden_private_file_if_exists(path)?;
    Ok(file)
}

pub fn permission_issue(path: &Path, kind: PrivatePathKind) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    #[cfg(unix)]
    {
        let mode = fs::metadata(path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            let expected = match kind {
                PrivatePathKind::File => "0600",
                PrivatePathKind::Directory => "0700",
            };
            return Ok(Some(format!(
                "path={} mode={mode:03o} expected owner-only permissions ({expected})",
                path.display()
            )));
        }
    }

    Ok(None)
}

pub fn runtime_secret_permission_issues(paths: &MoonPaths) -> Result<Vec<String>> {
    let auth_dir = paths.moon_home.join("auth");
    let checks = [
        (
            paths.moon_home.join(".env"),
            "runtime env file",
            PrivatePathKind::File,
        ),
        (auth_dir.clone(), "auth dir", PrivatePathKind::Directory),
        (
            auth_dir.join("openai-codex.json"),
            "managed oauth store",
            PrivatePathKind::File,
        ),
        (
            paths.logs_dir.clone(),
            "logs dir",
            PrivatePathKind::Directory,
        ),
        (
            paths.logs_dir.join("audit.log"),
            "audit log",
            PrivatePathKind::File,
        ),
        (
            paths.logs_dir.join("distill.audit.log"),
            "distill audit log",
            PrivatePathKind::File,
        ),
    ];

    let mut issues = Vec::new();
    for (path, label, kind) in checks {
        if let Some(issue) = permission_issue(&path, kind)? {
            issues.push(format!("{label}: insecure permissions ({issue})"));
        }
    }
    Ok(issues)
}
