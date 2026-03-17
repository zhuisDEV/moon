use crate::moon::config::{
    MoonContextCompactionAuthority, MoonContextConfig, MoonContextWindowMode,
};
use crate::openclaw::paths::{OpenClawPaths, ensure_parent_dir, normalize_path_for_storage};
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::NamedTempFile;

pub const MIN_AGENT_CONTEXT_TOKENS: u64 = 16_000;
// OpenClaw currently validates compaction mode as `default|safeguard`.
// Moon authority therefore uses `default` as the least-opinionated, valid mode.
pub const MOON_AUTHORITY_COMPACTION_MODE: &str = "default";
pub const OPENCLAW_AUTHORITY_COMPACTION_MODE: &str = "safeguard";

#[derive(Debug, Clone, Default)]
pub struct ConfigPatchOutcome {
    pub changed: bool,
    pub inserted_paths: Vec<String>,
    pub forced_paths: Vec<String>,
    pub removed_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ConfigPatchOptions {
    pub force: bool,
}

fn as_object_mut(value: &mut Value) -> Option<&mut Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn parse_config_text(raw: &str) -> Result<Value> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }

    match serde_json::from_str::<Value>(raw) {
        Ok(v) => Ok(v),
        Err(_) => json5::from_str::<Value>(raw).context("failed to parse config as JSON/JSON5"),
    }
}

pub fn read_config_value(paths: &OpenClawPaths) -> Result<Value> {
    if !paths.config_path.exists() {
        return Ok(json!({}));
    }
    let raw = fs::read_to_string(&paths.config_path)
        .with_context(|| format!("failed reading {}", paths.config_path.display()))?;
    parse_config_text(&raw)
}

fn set_path_if_absent_or_forced(
    root: &mut Value,
    path: &[&str],
    value: Value,
    force: bool,
    outcome: &mut ConfigPatchOutcome,
) {
    if path.is_empty() {
        return;
    }

    let mut cursor = root;
    for key in &path[..path.len() - 1] {
        if !cursor.is_object() {
            *cursor = Value::Object(Map::new());
        }
        let Some(map) = as_object_mut(cursor) else {
            return;
        };
        cursor = map
            .entry((*key).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }

    let leaf = path[path.len() - 1];
    if !cursor.is_object() {
        *cursor = Value::Object(Map::new());
    }
    let Some(map) = as_object_mut(cursor) else {
        return;
    };

    if let Some(existing) = map.get(leaf) {
        if force && existing != &value {
            map.insert(leaf.to_string(), value);
            outcome.changed = true;
            outcome.forced_paths.push(path.join("."));
        }
        return;
    }

    map.insert(leaf.to_string(), value);
    outcome.changed = true;
    outcome.inserted_paths.push(path.join("."));
}

fn remove_path(root: &mut Value, path: &[&str], outcome: &mut ConfigPatchOutcome) {
    if path.is_empty() {
        return;
    }
    let mut cursor = root;
    for key in &path[..path.len() - 1] {
        let Some(next) = cursor.get_mut(*key) else {
            return;
        };
        cursor = next;
    }
    let Some(map) = cursor.as_object_mut() else {
        return;
    };
    let leaf = path[path.len() - 1];
    if map.remove(leaf).is_some() {
        outcome.changed = true;
        outcome.removed_paths.push(path.join("."));
    }
}

fn patch_channel_limits(root: &mut Value, force: bool, outcome: &mut ConfigPatchOutcome) {
    let Some(channels) = root.get_mut("channels") else {
        return;
    };
    let Some(channels_map) = channels.as_object_mut() else {
        return;
    };

    for (provider, provider_cfg) in channels_map.iter_mut() {
        let Some(provider_map) = provider_cfg.as_object_mut() else {
            continue;
        };

        for (key, default_value) in [("historyLimit", 50), ("dmHistoryLimit", 30)] {
            if !provider_map.contains_key(key) || force {
                let existing = provider_map.get(key).cloned();
                if existing.is_none() {
                    provider_map.insert(key.to_string(), Value::from(default_value));
                    outcome.changed = true;
                    outcome
                        .inserted_paths
                        .push(format!("channels.{provider}.{key}"));
                } else if force && existing != Some(Value::from(default_value)) {
                    provider_map.insert(key.to_string(), Value::from(default_value));
                    outcome.changed = true;
                    outcome
                        .forced_paths
                        .push(format!("channels.{provider}.{key}"));
                }
            }
        }
    }
}

fn set_path_with_prefix(
    root: &mut Value,
    prefix: &[&str],
    suffix: &[&str],
    value: Value,
    force: bool,
    outcome: &mut ConfigPatchOutcome,
) {
    let mut path = Vec::with_capacity(prefix.len() + suffix.len());
    path.extend_from_slice(prefix);
    path.extend_from_slice(suffix);
    set_path_if_absent_or_forced(root, &path, value, force, outcome);
}

fn patch_plugin_token_defaults(
    root: &mut Value,
    plugin_id: &str,
    force: bool,
    outcome: &mut ConfigPatchOutcome,
) {
    let prefix = ["plugins", "entries", plugin_id, "config"];
    for (key, value) in [
        ("maxTokens", 12_000),
        ("maxChars", 60_000),
        ("maxRetainedBytes", 250_000),
    ] {
        set_path_with_prefix(root, &prefix, &[key], Value::from(value), force, outcome);
    }

    for (tool, max_tokens, max_chars) in [
        ("read", 6_000, 32_000),
        ("message/readMessages", 5_000, 28_000),
        ("message/searchMessages", 5_000, 28_000),
        ("web_fetch", 7_000, 35_000),
        ("web.fetch", 7_000, 35_000),
    ] {
        set_path_with_prefix(
            root,
            &prefix,
            &["tools", tool, "maxTokens"],
            Value::from(max_tokens),
            force,
            outcome,
        );
        set_path_with_prefix(
            root,
            &prefix,
            &["tools", tool, "maxChars"],
            Value::from(max_chars),
            force,
            outcome,
        );
    }
}

fn patch_context_policy(
    root: &mut Value,
    context: &MoonContextConfig,
    outcome: &mut ConfigPatchOutcome,
) {
    match context.window_mode {
        MoonContextWindowMode::Inherit => {
            remove_path(root, &["agents", "defaults", "contextTokens"], outcome);
        }
        MoonContextWindowMode::Fixed => {
            if let Some(tokens) = context.window_tokens {
                set_path_if_absent_or_forced(
                    root,
                    &["agents", "defaults", "contextTokens"],
                    Value::from(tokens),
                    true,
                    outcome,
                );
            }
        }
    }

    remove_path(root, &["agents", "defaults", "contextPruning"], outcome);

    let mode = match context.compaction_authority {
        MoonContextCompactionAuthority::Moon => MOON_AUTHORITY_COMPACTION_MODE,
        MoonContextCompactionAuthority::Openclaw => OPENCLAW_AUTHORITY_COMPACTION_MODE,
    };
    set_path_if_absent_or_forced(
        root,
        &["agents", "defaults", "compaction", "mode"],
        Value::from(mode),
        true,
        outcome,
    );
}

pub fn apply_config_patches(
    root: &mut Value,
    opts: &ConfigPatchOptions,
    plugin_id: &str,
    context_policy: Option<&MoonContextConfig>,
) -> ConfigPatchOutcome {
    if !root.is_object() {
        *root = json!({});
    }

    let mut outcome = ConfigPatchOutcome::default();

    if let Some(context) = context_policy {
        patch_context_policy(root, context, &mut outcome);
    } else {
        remove_path(
            root,
            &["agents", "defaults", "contextPruning"],
            &mut outcome,
        );
    }

    patch_channel_limits(root, opts.force, &mut outcome);
    patch_plugin_token_defaults(root, plugin_id, opts.force, &mut outcome);

    outcome
}

pub fn ensure_plugin_enabled(root: &mut Value, plugin_id: &str) -> ConfigPatchOutcome {
    let mut outcome = ConfigPatchOutcome::default();

    set_path_if_absent_or_forced(
        root,
        &["plugins", "entries", plugin_id, "enabled"],
        Value::Bool(true),
        true,
        &mut outcome,
    );

    outcome
}

pub fn ensure_plugin_install_record(
    root: &mut Value,
    plugin_id: &str,
    plugin_dir: &Path,
) -> ConfigPatchOutcome {
    let mut outcome = ConfigPatchOutcome::default();
    let plugin_dir_value = normalize_path_for_storage(plugin_dir).display().to_string();

    // Keep installs metadata aligned with the managed extension path so OpenClaw
    // can treat this plugin as provenance-tracked local code.
    set_path_if_absent_or_forced(
        root,
        &["plugins", "installs", plugin_id, "source"],
        Value::from("path"),
        true,
        &mut outcome,
    );
    set_path_if_absent_or_forced(
        root,
        &["plugins", "installs", plugin_id, "sourcePath"],
        Value::from(plugin_dir_value.clone()),
        true,
        &mut outcome,
    );
    set_path_if_absent_or_forced(
        root,
        &["plugins", "installs", plugin_id, "installPath"],
        Value::from(plugin_dir_value),
        true,
        &mut outcome,
    );

    outcome
}

pub fn ensure_plugin_slot(
    root: &mut Value,
    slot_name: &str,
    plugin_id: &str,
) -> ConfigPatchOutcome {
    let mut outcome = ConfigPatchOutcome::default();

    set_path_if_absent_or_forced(
        root,
        &["plugins", "slots", slot_name],
        Value::from(plugin_id),
        true,
        &mut outcome,
    );

    outcome
}

pub fn ensure_plugin_runtime_config(
    root: &mut Value,
    plugin_id: &str,
    moon_path: &Path,
    moon_home: &Path,
) -> ConfigPatchOutcome {
    let mut outcome = ConfigPatchOutcome::default();
    let prefix = ["plugins", "entries", plugin_id, "config"];
    let normalized_moon_path = normalize_path_for_storage(moon_path);
    let normalized_moon_home = normalize_path_for_storage(moon_home);

    set_path_with_prefix(
        root,
        &prefix,
        &["moonPath"],
        Value::from(normalized_moon_path.display().to_string()),
        true,
        &mut outcome,
    );
    set_path_with_prefix(
        root,
        &prefix,
        &["moonHome"],
        Value::from(normalized_moon_home.display().to_string()),
        true,
        &mut outcome,
    );
    set_path_with_prefix(
        root,
        &prefix,
        &["memoryDir"],
        Value::from(
            normalize_path_for_storage(&moon_home.join("memory"))
                .display()
                .to_string(),
        ),
        true,
        &mut outcome,
    );
    set_path_with_prefix(
        root,
        &prefix,
        &["memoryFile"],
        Value::from(
            normalize_path_for_storage(&moon_home.join("MEMORY.md"))
                .display()
                .to_string(),
        ),
        true,
        &mut outcome,
    );
    set_path_with_prefix(
        root,
        &prefix,
        &["fallbackMode"],
        Value::from("disabled"),
        true,
        &mut outcome,
    );
    set_path_with_prefix(
        root,
        &prefix,
        &["compactFallbackOnSkip"],
        Value::from(false),
        true,
        &mut outcome,
    );

    outcome
}

fn backup_path(config_path: &Path) -> Result<String> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("clock before unix epoch")?
        .as_secs();
    Ok(format!("{}.bak.{ts}", config_path.display()))
}

pub fn write_config_atomic(paths: &OpenClawPaths, value: &Value) -> Result<String> {
    ensure_parent_dir(&paths.config_path)?;

    if paths.config_path.exists() {
        let backup = backup_path(&paths.config_path)?;
        fs::copy(&paths.config_path, &backup).with_context(|| {
            format!(
                "failed backing up config {} -> {}",
                paths.config_path.display(),
                backup
            )
        })?;
    }

    let parent = paths
        .config_path
        .parent()
        .context("config path has no parent")?;
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    use std::io::Write;
    temp.write_all(b"\n")?;
    temp.flush()?;

    temp.persist(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("failed persisting config atomically: {}", e.error))?;

    Ok(paths.config_path.display().to_string())
}
