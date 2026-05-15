use crate::openclaw::paths::{OpenClawPaths, normalize_path_for_storage};
use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct PluginInstallIndexRecord {
    pub source: Option<String>,
    pub source_path: Option<String>,
    pub install_path: Option<String>,
}

fn path_string(root: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = root;
    for part in path {
        cursor = cursor.get(*part)?;
    }
    cursor.as_str().map(str::to_string)
}

pub fn read_plugin_install_index_record(
    paths: &OpenClawPaths,
    plugin_id: &str,
) -> Result<PluginInstallIndexRecord> {
    if !paths.plugin_index_path.exists() {
        return Ok(PluginInstallIndexRecord::default());
    }

    let raw = std::fs::read_to_string(&paths.plugin_index_path)
        .with_context(|| format!("failed reading {}", paths.plugin_index_path.display()))?;
    let root: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed parsing {}", paths.plugin_index_path.display()))?;

    Ok(PluginInstallIndexRecord {
        source: path_string(&root, &["installRecords", plugin_id, "source"]),
        source_path: path_string(&root, &["installRecords", plugin_id, "sourcePath"]),
        install_path: path_string(&root, &["installRecords", plugin_id, "installPath"]),
    })
}

pub fn expected_plugin_source_path(paths: &OpenClawPaths) -> String {
    normalize_path_for_storage(&paths.plugin_source_dir)
        .display()
        .to_string()
}

pub fn expected_plugin_install_path(paths: &OpenClawPaths) -> String {
    normalize_path_for_storage(&paths.plugin_dir)
        .display()
        .to_string()
}

pub fn install_index_record_matches(paths: &OpenClawPaths) -> Result<bool> {
    let record = read_plugin_install_index_record(paths, &paths.plugin_id)?;
    Ok(record.source.as_deref() == Some("path")
        && record.source_path.as_deref() == Some(expected_plugin_source_path(paths).as_str())
        && record.install_path.as_deref() == Some(expected_plugin_install_path(paths).as_str()))
}
