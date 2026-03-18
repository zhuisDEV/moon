use crate::assets::plugin_asset_contents;
use anyhow::Result;
use serde_json::Value;
use std::fs;

use crate::openclaw::gateway;
use crate::openclaw::paths::OpenClawPaths;

#[derive(Debug, Clone, Default)]
pub struct PluginVerifyOutcome {
    pub present_on_disk: bool,
    pub listed_by_openclaw: bool,
    pub loaded_by_openclaw: bool,
    pub assets_match_local: bool,
    pub provenance_warning_detected: bool,
}

#[derive(Debug, Clone, Default)]
struct PluginListState {
    listed: bool,
    loaded: bool,
    provenance_warning_detected: bool,
    provenance_diagnostic_messages: Vec<String>,
}

pub fn verify_plugin(paths: &OpenClawPaths) -> Result<PluginVerifyOutcome> {
    let present_on_disk = paths.plugin_dir.join("index.js").exists()
        && paths.plugin_dir.join("openclaw.plugin.json").exists()
        && paths.plugin_dir.join("package.json").exists();

    let assets_match_local = if present_on_disk {
        plugin_assets_match_local(paths)
    } else {
        false
    };

    let mut list_state = match gateway::plugins_list_json() {
        Ok(raw) => parse_plugins_list_state(&raw, &paths.plugin_id),
        Err(_) => PluginListState::default(),
    };

    // Fallback to per-plugin info when list parsing/execution is degraded.
    // This keeps strict verify accurate even when `plugins list --json`
    // output format is noisy or too large for brittle integrations.
    if (!list_state.listed || !list_state.loaded)
        && let Ok(raw) = gateway::plugins_info_json(&paths.plugin_id)
        && let Some(info_state) = parse_plugin_info_state(&raw, &paths.plugin_id)
    {
        list_state.listed = info_state.listed;
        list_state.loaded = info_state.loaded;
    }

    Ok(PluginVerifyOutcome {
        present_on_disk,
        listed_by_openclaw: list_state.listed,
        loaded_by_openclaw: list_state.loaded,
        assets_match_local,
        provenance_warning_detected: list_state.provenance_warning_detected,
    })
}

fn parse_plugin_info_state(raw: &str, plugin_id: &str) -> Option<PluginListState> {
    let v = parse_json_value(raw)?;
    let id = v.get("id").and_then(Value::as_str).unwrap_or_default();
    if id != plugin_id {
        return None;
    }
    let loaded = v
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status == "loaded")
        .unwrap_or(true);
    Some(PluginListState {
        listed: true,
        loaded,
        provenance_warning_detected: false,
        provenance_diagnostic_messages: Vec::new(),
    })
}

fn plugin_assets_match_local(paths: &OpenClawPaths) -> bool {
    for (name, expected) in plugin_asset_contents() {
        let path = paths.plugin_dir.join(name);
        let Ok(current) = fs::read_to_string(path) else {
            return false;
        };
        if current != expected {
            return false;
        }
    }
    true
}

fn parse_plugins_list_state(raw: &str, plugin_id: &str) -> PluginListState {
    let Some(v) = parse_json_value(raw) else {
        return PluginListState::default();
    };

    let arr_opt = v
        .as_array()
        .or_else(|| v.get("plugins").and_then(Value::as_array));

    let mut state = PluginListState {
        provenance_diagnostic_messages: parse_provenance_diagnostics(&v, plugin_id),
        ..Default::default()
    };
    state.provenance_warning_detected = !state.provenance_diagnostic_messages.is_empty();

    let Some(arr) = arr_opt else {
        return state;
    };

    for entry in arr {
        let is_match = entry
            .get("id")
            .and_then(Value::as_str)
            .map(|id| id == plugin_id)
            .unwrap_or(false);
        if !is_match {
            continue;
        }
        state.listed = true;
        state.loaded = entry
            .get("status")
            .and_then(Value::as_str)
            .map(|status| status == "loaded")
            .unwrap_or(true);
        return state;
    }

    state
}

fn parse_json_value(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return Some(value);
    }

    // `openclaw plugins list --json` can include non-JSON preamble lines.
    // Instead of assuming the first `{` or `[` is JSON, scan every possible
    // JSON start and return the first successfully parsed value.
    for (idx, ch) in raw.char_indices() {
        if ch != '{' && ch != '[' {
            continue;
        }
        let candidate = &raw[idx..];
        let mut stream = serde_json::Deserializer::from_str(candidate).into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            return Some(value);
        }
    }

    None
}

fn parse_provenance_diagnostics(root: &Value, plugin_id: &str) -> Vec<String> {
    let Some(diagnostics) = root.get("diagnostics").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut matches = Vec::new();
    for diagnostic in diagnostics {
        let Some(message) = diagnostic.get("message").and_then(Value::as_str) else {
            continue;
        };
        if !diagnostic_targets_plugin(diagnostic, plugin_id) {
            continue;
        }
        if !is_provenance_warning_message(message) {
            continue;
        }
        matches.push(message.to_string());
    }
    matches
}

fn diagnostic_targets_plugin(diagnostic: &Value, plugin_id: &str) -> bool {
    if let Some(id) = diagnostic.get("pluginId").and_then(Value::as_str) {
        return id == plugin_id;
    }

    if let Some(source) = diagnostic.get("source").and_then(Value::as_str) {
        let marker = format!("/{plugin_id}/");
        if source.contains(&marker) {
            return true;
        }
    }

    diagnostic
        .get("message")
        .and_then(Value::as_str)
        .map(|message| {
            let lowered = message.to_ascii_lowercase();
            lowered.contains(&plugin_id.to_ascii_lowercase()) && lowered.contains("provenance")
        })
        .unwrap_or(false)
}

fn is_provenance_warning_message(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("loaded without install/load-path provenance") {
        return true;
    }

    lowered.contains("provenance")
        && (lowered.contains("untracked")
            || lowered.contains("install")
            || lowered.contains("load-path"))
}

#[cfg(test)]
mod tests {
    use super::parse_plugins_list_state;

    #[test]
    fn parse_plugins_list_state_tolerates_preamble_before_json() {
        let raw = r#"│
◇ Doctor changes ──────────────────────────────────────────────╮
│  Run "openclaw doctor --fix" to apply these changes.         │
├───────────────────────────────────────────────────────────────╯
{
  "plugins": [
    {"id":"moon","status":"loaded"}
  ],
  "diagnostics": []
}"#;

        let state = parse_plugins_list_state(raw, "moon");
        assert!(state.listed);
        assert!(state.loaded);
        assert!(!state.provenance_warning_detected);
    }
}
