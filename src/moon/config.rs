use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

mod generated_env_allowlist {
    include!(concat!(env!("OUT_DIR"), "/moon_env_allowlist.rs"));
}

pub const SECRET_ENV_KEYS: [&str; 4] = [
    "GEMINI_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "AI_API_KEY",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonThresholds {
    pub trigger_ratio: f64,
}

impl Default for MoonThresholds {
    fn default() -> Self {
        Self {
            trigger_ratio: 0.85,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonWatcherConfig {
    pub poll_interval_secs: u64,
    pub cooldown_secs: u64,
}

impl Default for MoonWatcherConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            cooldown_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonDistillConfig {
    pub max_per_cycle: u64,
    #[serde(default = "default_residential_timezone")]
    pub residential_timezone: String,
    #[serde(default)]
    pub topic_discovery: bool,
    #[serde(default)]
    pub chunk_bytes: Option<String>,
    #[serde(default)]
    pub max_chunks: Option<u64>,
    #[serde(default)]
    pub model_context_tokens: Option<u64>,
}

fn default_residential_timezone() -> String {
    "UTC".to_string()
}

impl Default for MoonDistillConfig {
    fn default() -> Self {
        Self {
            max_per_cycle: 1,
            residential_timezone: "UTC".to_string(),
            topic_discovery: false,
            chunk_bytes: None,
            max_chunks: None,
            model_context_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonEmbedConfig {
    pub mode: String,
    pub cooldown_secs: u64,
    pub max_docs_per_cycle: u64,
    pub min_pending_docs: u64,
    pub max_cycle_secs: u64,
}

impl Default for MoonEmbedConfig {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            cooldown_secs: 60,
            max_docs_per_cycle: 25,
            min_pending_docs: 1,
            max_cycle_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoonHotCollectionLifecycleMode {
    #[default]
    Degrade,
    Disabled,
    Strict,
}

impl MoonHotCollectionLifecycleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Degrade => "degrade",
            Self::Disabled => "disabled",
            Self::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoonHotCollectionLifecycleCommandMode {
    #[default]
    Primary,
    Fallback,
}

impl MoonHotCollectionLifecycleCommandMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonHotCollectionConfig {
    pub lifecycle_mode: MoonHotCollectionLifecycleMode,
    pub lifecycle_command_mode: MoonHotCollectionLifecycleCommandMode,
}

impl Default for MoonHotCollectionConfig {
    fn default() -> Self {
        Self {
            lifecycle_mode: MoonHotCollectionLifecycleMode::Degrade,
            lifecycle_command_mode: MoonHotCollectionLifecycleCommandMode::Primary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoonContextWindowMode {
    #[default]
    Inherit,
    Fixed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoonContextCompactionAuthority {
    #[default]
    Moon,
    Openclaw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MoonContextConfig {
    pub window_mode: MoonContextWindowMode,
    pub window_tokens: Option<u64>,
    pub compaction_authority: MoonContextCompactionAuthority,
    #[serde(alias = "compaction_start_ratio")]
    pub cleanse_trigger_ratio: f64,
    #[serde(alias = "compaction_emergency_ratio")]
    pub cleanse_emergency_ratio: f64,
    pub compaction_recover_ratio: f64,
}

impl Default for MoonContextConfig {
    fn default() -> Self {
        Self {
            window_mode: MoonContextWindowMode::Inherit,
            window_tokens: None,
            compaction_authority: MoonContextCompactionAuthority::Moon,
            cleanse_trigger_ratio: 0.50,
            cleanse_emergency_ratio: 0.90,
            // Reserved for future policy extensions; current trigger logic
            // does not depend on recover ratio.
            compaction_recover_ratio: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MoonConfig {
    pub thresholds: MoonThresholds,
    pub watcher: MoonWatcherConfig,
    pub distill: MoonDistillConfig,
    pub embed: MoonEmbedConfig,
    pub hot_collection: MoonHotCollectionConfig,
    pub context: Option<MoonContextConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialMoonConfig {
    thresholds: Option<PartialMoonThresholds>,
    watcher: Option<MoonWatcherConfig>,
    distill: Option<MoonDistillConfig>,
    embed: Option<MoonEmbedConfig>,
    hot_collection: Option<MoonHotCollectionConfig>,
    context: Option<MoonContextConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialMoonThresholds {
    trigger_ratio: Option<f64>,
    archive_ratio: Option<f64>,
    #[serde(alias = "prune_ratio")]
    compaction_ratio: Option<f64>,
    #[serde(rename = "archive_ratio_trigger_enabled")]
    _archive_ratio_trigger_enabled: Option<bool>,
}

fn env_or_f64_first(vars: &[&str], fallback: f64) -> f64 {
    for var in vars {
        if let Ok(v) = env::var(var) {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(parsed) = trimmed.parse::<f64>() {
                return parsed;
            }
        }
    }
    fallback
}

fn env_or_u64(var: &str, fallback: u64) -> u64 {
    match env::var(var) {
        Ok(v) => v.trim().parse::<u64>().ok().unwrap_or(fallback),
        Err(_) => fallback,
    }
}

fn env_or_bool(var: &str, fallback: bool) -> bool {
    match env::var(var) {
        Ok(v) => {
            let trimmed = v.trim();
            match trimmed {
                "1" | "true" | "TRUE" | "yes" | "on" => true,
                "0" | "false" | "FALSE" | "no" | "off" => false,
                _ => fallback,
            }
        }
        Err(_) => fallback,
    }
}

fn env_or_string(var: &str, fallback: &str) -> String {
    match env::var(var) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => fallback.to_string(),
    }
}

fn normalize_embed_mode(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("auto") {
        "auto".to_string()
    } else {
        raw.trim().to_string()
    }
}

pub fn resolve_hot_collection_lifecycle_policy_for_diagnostics() -> (
    MoonHotCollectionLifecycleMode,
    MoonHotCollectionLifecycleCommandMode,
    Option<String>,
) {
    match load_config() {
        Ok(cfg) => (
            cfg.hot_collection.lifecycle_mode,
            cfg.hot_collection.lifecycle_command_mode,
            None,
        ),
        Err(err) => (
            MoonHotCollectionLifecycleMode::Degrade,
            MoonHotCollectionLifecycleCommandMode::Primary,
            Some(format!("config-load-failed err={err:#}")),
        ),
    }
}

fn validate(cfg: &MoonConfig) -> Result<()> {
    let trigger = cfg.thresholds.trigger_ratio;
    if !(trigger > 0.0 && trigger <= 1.0) {
        return Err(anyhow!("invalid trigger ratio: require 0 < trigger <= 1.0"));
    }
    if cfg.watcher.poll_interval_secs == 0 {
        return Err(anyhow!(
            "invalid watcher poll interval: must be >= 1 second"
        ));
    }
    if cfg.distill.max_per_cycle == 0 {
        return Err(anyhow!("invalid distill max per cycle: must be >= 1"));
    }
    if let Some(max_chunks) = cfg.distill.max_chunks
        && max_chunks == 0
    {
        return Err(anyhow!("invalid distill max_chunks: must be >= 1"));
    }
    if let Some(chunk_bytes) = &cfg.distill.chunk_bytes {
        let trimmed = chunk_bytes.trim();
        if !trimmed.is_empty()
            && !trimmed.eq_ignore_ascii_case("auto")
            && trimmed.parse::<usize>().ok().filter(|v| *v > 0).is_none()
        {
            return Err(anyhow!(
                "invalid distill chunk_bytes: use `auto` or a positive integer"
            ));
        }
    }
    if cfg.embed.mode != "auto" {
        return Err(anyhow!("invalid embed mode: use `auto`"));
    }
    if cfg.embed.cooldown_secs == 0 {
        return Err(anyhow!("invalid embed cooldown secs: must be >= 1"));
    }
    if cfg.embed.max_docs_per_cycle == 0 {
        return Err(anyhow!("invalid embed max docs per cycle: must be >= 1"));
    }
    if cfg.embed.min_pending_docs == 0 {
        return Err(anyhow!("invalid embed min pending docs: must be >= 1"));
    }
    if cfg.embed.max_cycle_secs == 0 {
        return Err(anyhow!("invalid embed max cycle secs: must be >= 1"));
    }
    if let Some(context) = &cfg.context {
        if matches!(context.window_mode, MoonContextWindowMode::Fixed) {
            let Some(window_tokens) = context.window_tokens else {
                return Err(anyhow!(
                    "invalid context config: window_tokens is required when window_mode=fixed"
                ));
            };
            if window_tokens < 16_000 {
                return Err(anyhow!(
                    "invalid context config: window_tokens must be >= 16000 when window_mode=fixed"
                ));
            }
        }
        if !(context.cleanse_trigger_ratio > 0.0 && context.cleanse_trigger_ratio <= 1.0) {
            return Err(anyhow!(
                "invalid context config: require 0 < cleanse_trigger_ratio <= 1.0"
            ));
        }
        if !(context.cleanse_emergency_ratio > 0.0 && context.cleanse_emergency_ratio <= 1.0) {
            return Err(anyhow!(
                "invalid context config: require 0 < cleanse_emergency_ratio <= 1.0"
            ));
        }
        if !(context.compaction_recover_ratio >= 0.0 && context.compaction_recover_ratio < 1.0) {
            return Err(anyhow!(
                "invalid context config: require 0 <= compaction_recover_ratio < 1.0"
            ));
        }
        if context.cleanse_trigger_ratio > context.cleanse_emergency_ratio {
            return Err(anyhow!(
                "invalid context config: require cleanse_trigger_ratio <= cleanse_emergency_ratio"
            ));
        }
    }
    Ok(())
}

fn config_path_candidates(
    moon_config_path: Option<PathBuf>,
    moon_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(path) = moon_config_path {
        return vec![path];
    }

    if let Some(home) = moon_home {
        return vec![home.join("moon.toml")];
    }

    let Some(home) = home_dir else {
        return Vec::new();
    };
    vec![home.join(".moon").join("moon.toml")]
}

fn resolve_config_paths() -> Vec<PathBuf> {
    let moon_config_path = env::var("MOON_CONFIG_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let moon_home = env::var("MOON_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    config_path_candidates(moon_config_path, moon_home, dirs::home_dir())
}

pub fn resolve_config_path() -> Option<PathBuf> {
    let candidates = resolve_config_paths();
    if let Some(existing) = candidates.iter().find(|path| path.exists()) {
        return Some(existing.clone());
    }
    candidates.into_iter().next()
}

fn merge_file_config(base: &mut MoonConfig) -> Result<()> {
    let paths = resolve_config_paths();
    if paths.is_empty() {
        return Ok(());
    }

    for path in paths {
        if !path.exists() {
            continue;
        }

        let raw = fs::read_to_string(&path)?;
        let parsed: PartialMoonConfig = toml::from_str(&raw)
            .map_err(|err| anyhow!("failed to parse moon config {}: {err}", path.display()))?;
        if let Some(thresholds) = parsed.thresholds
            && let Some(trigger_ratio) = thresholds
                .trigger_ratio
                .or(thresholds.compaction_ratio)
                .or(thresholds.archive_ratio)
        {
            base.thresholds.trigger_ratio = trigger_ratio;
        }
        if let Some(watcher) = parsed.watcher {
            base.watcher = watcher;
        }
        if let Some(distill) = parsed.distill {
            base.distill = distill;
        }
        if let Some(embed) = parsed.embed {
            base.embed = embed;
        }
        if let Some(hot_collection) = parsed.hot_collection {
            base.hot_collection = hot_collection;
        }
        if let Some(context) = parsed.context {
            base.context = Some(context);
        }
        break;
    }

    Ok(())
}

pub fn load_config() -> Result<MoonConfig> {
    let mut cfg = MoonConfig::default();
    merge_file_config(&mut cfg)?;

    cfg.thresholds.trigger_ratio =
        env_or_f64_first(&["MOON_TRIGGER_RATIO"], cfg.thresholds.trigger_ratio);
    cfg.watcher.poll_interval_secs =
        env_or_u64("MOON_POLL_INTERVAL_SECS", cfg.watcher.poll_interval_secs);
    cfg.watcher.cooldown_secs = env_or_u64("MOON_COOLDOWN_SECS", cfg.watcher.cooldown_secs);
    cfg.distill.max_per_cycle = env_or_u64("MOON_DISTILL_MAX_PER_CYCLE", cfg.distill.max_per_cycle);
    cfg.distill.residential_timezone = env_or_string(
        "MOON_RESIDENTIAL_TIMEZONE",
        &cfg.distill.residential_timezone,
    );
    cfg.distill.topic_discovery = env_or_bool("MOON_TOPIC_DISCOVERY", cfg.distill.topic_discovery);
    cfg.embed.mode = env_or_string("MOON_EMBED_MODE", &cfg.embed.mode);
    cfg.embed.cooldown_secs = env_or_u64("MOON_EMBED_COOLDOWN_SECS", cfg.embed.cooldown_secs);
    cfg.embed.max_docs_per_cycle = env_or_u64(
        "MOON_EMBED_MAX_DOCS_PER_CYCLE",
        cfg.embed.max_docs_per_cycle,
    );
    cfg.embed.min_pending_docs =
        env_or_u64("MOON_EMBED_MIN_PENDING_DOCS", cfg.embed.min_pending_docs);
    cfg.embed.max_cycle_secs = env_or_u64("MOON_EMBED_MAX_CYCLE_SECS", cfg.embed.max_cycle_secs);
    cfg.embed.mode = normalize_embed_mode(&cfg.embed.mode);

    validate(&cfg)?;
    audit_env_vars();
    Ok(cfg)
}

pub fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return "[UNSET]".to_string();
    }

    let chars = trimmed.chars().collect::<Vec<_>>();
    if chars.len() < 8 {
        return "[SET]".to_string();
    }

    let first3 = chars.iter().take(3).collect::<String>();
    let last4 = chars[chars.len().saturating_sub(4)..]
        .iter()
        .collect::<String>();
    format!("{first3}...{last4}")
}

pub fn masked_env_secret(var: &str) -> String {
    match env::var(var) {
        Ok(v) => mask_secret(&v),
        Err(_) => "[UNSET]".to_string(),
    }
}

fn env_allowlist() -> &'static [&'static str] {
    generated_env_allowlist::GENERATED_MOON_ENV_ALLOWLIST
}

fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut prev_row = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut curr_row = vec![0usize; right_chars.len() + 1];

    for (i, lc) in left_chars.iter().enumerate() {
        curr_row[0] = i + 1;
        for (j, rc) in right_chars.iter().enumerate() {
            let cost = if lc == rc { 0 } else { 1 };
            curr_row[j + 1] = std::cmp::min(
                std::cmp::min(curr_row[j] + 1, prev_row[j + 1] + 1),
                prev_row[j] + cost,
            );
        }
        prev_row.clone_from_slice(&curr_row);
    }

    prev_row[right_chars.len()]
}

fn nearest_allowed_env_key<'a>(candidate: &str, allowlist: &'a [&str]) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for allowed in allowlist {
        let distance = levenshtein_distance(candidate, allowed);
        match best {
            Some((best_distance, _)) if distance >= best_distance => {}
            _ => best = Some((distance, allowed)),
        }
    }
    let (distance, key) = best?;
    if distance <= 4 { Some(key) } else { None }
}

fn audit_env_vars() {
    let allowlist = env_allowlist();

    for (key, _) in env::vars() {
        if key.starts_with("MOON_") && !allowlist.contains(&key.as_str()) {
            if let Some(suggestion) = nearest_allowed_env_key(&key, allowlist) {
                eprintln!(
                    "WARN: unrecognized environment variable: {key}. Did you mean `{suggestion}`?"
                );
            } else {
                eprintln!("WARN: unrecognized environment variable: {key}");
            }
        }
    }
}

fn has_explicit_context_policy_env() -> bool {
    for var in ["MOON_CONFIG_PATH", "MOON_HOME"] {
        if let Ok(v) = env::var(var)
            && !v.trim().is_empty()
        {
            return true;
        }
    }
    false
}

pub fn load_context_policy_if_explicit_env() -> Result<Option<MoonContextConfig>> {
    if !has_explicit_context_policy_env() {
        return Ok(None);
    }
    Ok(load_config()?.context)
}

#[cfg(test)]
mod tests {
    use super::{config_path_candidates, load_config, mask_secret};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnvVar {
        key: &'static str,
        previous: Option<String>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                unsafe {
                    std::env::set_var(self.key, previous);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn mask_secret_unset_and_short_values() {
        assert_eq!(mask_secret(""), "[UNSET]");
        assert_eq!(mask_secret("short"), "[SET]");
    }

    #[test]
    fn mask_secret_keeps_prefix_and_suffix() {
        assert_eq!(mask_secret("sk-1234567890abcdef"), "sk-...cdef");
    }

    #[test]
    fn config_path_prefers_explicit_moon_config_path() {
        let got = config_path_candidates(
            Some(PathBuf::from("/tmp/custom.toml")),
            Some(PathBuf::from("/workspace")),
            Some(PathBuf::from("/home/alice")),
        );
        assert_eq!(got, vec![PathBuf::from("/tmp/custom.toml")]);
    }

    #[test]
    fn config_path_uses_moon_home_when_set() {
        let got = config_path_candidates(
            None,
            Some(PathBuf::from("/workspace")),
            Some(PathBuf::from("/home/alice")),
        );
        assert_eq!(got, vec![PathBuf::from("/workspace/moon.toml")]);
    }

    #[test]
    fn config_path_defaults_to_dot_moon_home() {
        let got = config_path_candidates(None, None, Some(PathBuf::from("/home/alice")));
        assert_eq!(got, vec![PathBuf::from("/home/alice/.moon/moon.toml"),]);
    }

    #[test]
    fn moon_toml_overrides_hot_collection_lifecycle_policy() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        std::fs::create_dir_all(&moon_home).expect("mkdir moon home");
        let _home = ScopedEnvVar::set("MOON_HOME", &moon_home.display().to_string());
        std::fs::write(
            moon_home.join("moon.toml"),
            r#"[hot_collection]
lifecycle_mode = "strict"
lifecycle_command_mode = "fallback"
"#,
        )
        .expect("write moon.toml");

        let cfg = load_config().expect("load config");
        assert_eq!(
            cfg.hot_collection.lifecycle_mode,
            super::MoonHotCollectionLifecycleMode::Strict
        );
        assert_eq!(
            cfg.hot_collection.lifecycle_command_mode,
            super::MoonHotCollectionLifecycleCommandMode::Fallback
        );
    }

    #[test]
    fn moon_toml_accepts_disabled_hot_collection_lifecycle_mode() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        std::fs::create_dir_all(&moon_home).expect("mkdir moon home");
        let _home = ScopedEnvVar::set("MOON_HOME", &moon_home.display().to_string());
        std::fs::write(
            moon_home.join("moon.toml"),
            r#"[hot_collection]
lifecycle_mode = "disabled"
"#,
        )
        .expect("write moon.toml");

        let cfg = load_config().expect("load config");
        assert_eq!(
            cfg.hot_collection.lifecycle_mode,
            super::MoonHotCollectionLifecycleMode::Disabled
        );
    }

    #[test]
    fn invalid_hot_collection_lifecycle_mode_is_rejected() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        std::fs::create_dir_all(&moon_home).expect("mkdir moon home");
        let _home = ScopedEnvVar::set("MOON_HOME", &moon_home.display().to_string());
        std::fs::write(
            moon_home.join("moon.toml"),
            r#"[hot_collection]
lifecycle_mode = "invalid-mode"
"#,
        )
        .expect("write moon.toml");

        let err = load_config().expect_err("invalid mode should fail");
        assert!(format!("{err:#}").contains("invalid-mode"), "{err:#}");
    }

    #[test]
    fn invalid_hot_collection_lifecycle_command_mode_is_rejected() {
        let _lock = TEST_ENV_LOCK.lock().expect("env lock");
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        std::fs::create_dir_all(&moon_home).expect("mkdir moon home");
        let _home = ScopedEnvVar::set("MOON_HOME", &moon_home.display().to_string());
        std::fs::write(
            moon_home.join("moon.toml"),
            r#"[hot_collection]
lifecycle_command_mode = "broken"
"#,
        )
        .expect("write moon.toml");

        let err = load_config().expect_err("invalid command mode should fail");
        assert!(format!("{err:#}").contains("broken"), "{err:#}");
    }
}
