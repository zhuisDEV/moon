use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn ensure_readable_file(path: &Path, command_name: &str) -> Result<()> {
    if !path.is_file() {
        anyhow::bail!(
            "{command_name} source is not a readable file: {}",
            path.display()
        );
    }
    let _ = fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    Ok(())
}

pub fn resolve_session_id(explicit: Option<&str>, path: &Path) -> Result<String> {
    if let Some(session_id) = explicit
        && !session_id.trim().is_empty()
    {
        return Ok(session_id.trim().to_string());
    }

    path.file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow::anyhow!("failed to derive session id from {}", path.display()))
}

pub fn file_epoch_secs(path: &Path) -> u64 {
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return 0;
    };
    duration.as_secs()
}

pub fn gather_files_with_extension(
    root: &Path,
    extension: &str,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {}", path.display()))?;
        if file_type.is_dir() {
            if recursive {
                gather_files_with_extension(&path, extension, recursive, out)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        {
            out.push(path);
        }
    }
    Ok(())
}

pub fn latest_file_with_extension(
    root: &Path,
    extension: &str,
    recursive: bool,
) -> Result<Option<PathBuf>> {
    let mut files = Vec::new();
    gather_files_with_extension(root, extension, recursive, &mut files)?;
    Ok(files.into_iter().max_by(|left, right| {
        file_epoch_secs(left)
            .cmp(&file_epoch_secs(right))
            .then_with(|| left.cmp(right))
    }))
}
