use crate::model::{ImportReport, IngestDocument, LegacySearchHit};
use crate::store::Store;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const MAX_LEGACY_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
struct LegacySource {
    path: PathBuf,
    source_kind: &'static str,
}

pub fn search_legacy(
    source_root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<LegacySearchHit>> {
    let source_root = source_root
        .canonicalize()
        .with_context(|| format!("failed to resolve legacy root {}", source_root.display()))?;
    let terms = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| term.chars().count() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        anyhow::bail!("shadow query contains no searchable terms");
    }
    let mut sources = Vec::new();
    add_file(&mut sources, source_root.join("MEMORY.md"), "legacy-memory");
    add_tree(
        &mut sources,
        &source_root.join("memory"),
        "memory-daily",
        &["md"],
    );
    add_tree(
        &mut sources,
        &source_root.join("mlib"),
        "library",
        &["md", "txt"],
    );
    add_tree(
        &mut sources,
        &source_root.join("mds"),
        "hot",
        &["md", "txt"],
    );
    let mut hits = Vec::new();
    for source in sources {
        if fs::metadata(&source.path).is_ok_and(|metadata| metadata.len() > MAX_LEGACY_FILE_BYTES) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&source.path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            let lowercase = line.to_ascii_lowercase();
            let score = terms
                .iter()
                .filter(|term| lowercase.contains(term.as_str()))
                .count();
            if score > 0 {
                hits.push(LegacySearchHit {
                    path: source.path.clone(),
                    line_number: index + 1,
                    text: line.trim().chars().take(320).collect(),
                    score,
                });
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line_number.cmp(&right.line_number))
    });
    hits.truncate(limit.clamp(1, 100));
    Ok(hits)
}

pub fn import_legacy(
    store: &mut Store,
    source_root: &Path,
    include_raw: bool,
    dry_run: bool,
) -> Result<ImportReport> {
    let source_root = source_root
        .canonicalize()
        .with_context(|| format!("failed to resolve legacy root {}", source_root.display()))?;
    let mut sources = Vec::new();
    add_file(&mut sources, source_root.join("MEMORY.md"), "legacy-memory");
    add_tree(
        &mut sources,
        &source_root.join("memory"),
        "memory-daily",
        &["md"],
    );
    add_tree(
        &mut sources,
        &source_root.join("mlib"),
        "library",
        &["md", "txt"],
    );
    add_tree(
        &mut sources,
        &source_root.join("mds"),
        "hot",
        &["md", "txt"],
    );
    if include_raw {
        add_tree(&mut sources, &source_root.join("raw"), "raw", &["jsonl"]);
    }
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    sources.dedup_by(|left, right| left.path == right.path);

    let mut report = ImportReport {
        source_root: source_root.clone(),
        discovered: sources.len(),
        ..ImportReport::default()
    };

    for source in sources {
        let result = (|| -> Result<bool> {
            let metadata = fs::metadata(&source.path)
                .with_context(|| format!("failed to stat {}", source.path.display()))?;
            if metadata.len() > MAX_LEGACY_FILE_BYTES {
                anyhow::bail!(
                    "file exceeds the maximum import size of {MAX_LEGACY_FILE_BYTES} bytes"
                );
            }
            let content = fs::read_to_string(&source.path)
                .with_context(|| format!("failed to read {}", source.path.display()))?;
            if dry_run {
                return Ok(true);
            }
            let modified_at_ms = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let title = source
                .path
                .file_stem()
                .and_then(|value| value.to_str())
                .map(ToString::to_string);
            let outcome = store.ingest(IngestDocument {
                source_uri: format!("legacy://{}", source.path.display()),
                source_kind: source.source_kind.to_string(),
                scope: "legacy".to_string(),
                title,
                content,
                modified_at_ms,
                metadata_json: serde_json::json!({
                    "legacy_path": source.path,
                    "imported_read_only": true
                })
                .to_string(),
            })?;
            Ok(outcome.changed)
        })();

        match result {
            Ok(true) => report.imported += 1,
            Ok(false) => report.unchanged += 1,
            Err(err) => {
                report.failed += 1;
                report
                    .failures
                    .push(format!("{}: {err:#}", source.path.display()));
            }
        }
    }
    Ok(report)
}

fn add_file(sources: &mut Vec<LegacySource>, path: PathBuf, source_kind: &'static str) {
    if path.is_file() {
        sources.push(LegacySource { path, source_kind });
    }
}

fn add_tree(
    sources: &mut Vec<LegacySource>,
    root: &Path,
    source_kind: &'static str,
    extensions: &[&str],
) {
    if !root.is_dir() {
        return;
    }
    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let matches = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| extensions.iter().any(|ext| value.eq_ignore_ascii_case(ext)));
        if matches {
            sources.push(LegacySource {
                path: entry.path().to_path_buf(),
                source_kind,
            });
        }
    }
}
