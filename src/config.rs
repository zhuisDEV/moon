//! Runtime learning configuration, independent of installed release files.
//!
//! Authentication stays with OpenClaw. Neither credentials nor arbitrary
//! provider options are accepted in this file.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// A starter file deliberately leaves daily synthesis disabled until its model
/// route has been verified. Built-in prompts remain in use unless overridden.
const STARTER_CONFIG: &str = r#"# Moon learning configuration. Authentication is provided by OpenClaw.
# Relative prompt_file paths are resolved against this file's directory.
# max_output_tokens is a provider request; native Codex does not enforce it.
# Moon enforces input, timeout and attempt limits independently.

[learning]
observation_ttl_hours = 24

[learning.l1]
enabled = true
model = "openai/gpt-6-astra"
reasoning = "low"
timeout_ms = 120000
max_output_tokens = 8192
# fallback_model = "provider/model"
# fallback_reasoning = "low"
# fallback_enabled = false
# prompt_file = "prompts/l1-distill.md"

[learning.l2]
# Enable after verifying the selected OpenClaw model and reviewing a dry run.
enabled = false
model = "openai/gpt-6-astra"
reasoning = "xhigh"
timeout_ms = 600000
max_output_tokens = 32768
daily_at = "03:00"
timezone = "Australia/Sydney"
batch_size = 32
max_batches_per_day = 8
max_actions = 16
max_input_chars = 64000
# fallback_model = "provider/model"
# fallback_reasoning = "high"
# fallback_enabled = false
# prompt_file = "prompts/l2-synth.md"
"#;

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedConfig {
    pub path: PathBuf,
    pub present: bool,
    pub learning: LearningConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    learning: LearningConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LearningConfig {
    pub observation_ttl_hours: u32,
    pub l1: L1Config,
    pub l2: L2Config,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            observation_ttl_hours: 24,
            l1: L1Config::default(),
            l2: L2Config::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct L1Config {
    pub enabled: bool,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub fallback_model: Option<String>,
    pub fallback_reasoning: Option<String>,
    pub fallback_enabled: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub prompt_file: Option<PathBuf>,
}

impl Default for L1Config {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            reasoning: None,
            fallback_model: None,
            fallback_reasoning: None,
            fallback_enabled: None,
            timeout_ms: None,
            max_output_tokens: None,
            prompt_file: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct L2Config {
    pub enabled: bool,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub fallback_model: Option<String>,
    pub fallback_reasoning: Option<String>,
    pub fallback_enabled: Option<bool>,
    pub timeout_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub prompt_file: Option<PathBuf>,
    pub daily_at: String,
    /// Validated against IANA data by config validate and the adapter scheduler.
    pub timezone: String,
    pub batch_size: u32,
    pub max_batches_per_day: u32,
    pub max_actions: u32,
    pub max_input_chars: u32,
}

impl Default for L2Config {
    fn default() -> Self {
        Self {
            enabled: false,
            model: None,
            reasoning: None,
            fallback_model: None,
            fallback_reasoning: None,
            fallback_enabled: None,
            timeout_ms: None,
            max_output_tokens: None,
            prompt_file: None,
            daily_at: "03:00".to_owned(),
            timezone: "Australia/Sydney".to_owned(),
            batch_size: 32,
            max_batches_per_day: 8,
            max_actions: 16,
            max_input_chars: 64_000,
        }
    }
}

/// Load only `<home>/moon.toml`. An absent file preserves legacy routing through
/// optional model settings; it never creates the home or opens the database.
pub fn load(home: &Path) -> Result<ResolvedConfig> {
    let path = absolute_path(&home.join("moon.toml"))?;
    let file = match open_nonblocking(&path) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // A dangling symlink is a broken configuration, not an absent file.
            if fs::symlink_metadata(&path).is_ok() {
                bail!("moon.toml could not be read");
            }
            None
        }
        Err(_) => bail!("moon.toml could not be read"),
    };
    let present = file.is_some();
    let mut config = match file {
        Some(file) => {
            if !file
                .metadata()
                .context("could not inspect moon.toml")?
                .is_file()
            {
                bail!("moon.toml must be a regular file");
            }
            let mut bytes = Vec::new();
            file.take(MAX_CONFIG_BYTES + 1)
                .read_to_end(&mut bytes)
                .context("moon.toml could not be read")?;
            if bytes.len() as u64 > MAX_CONFIG_BYTES {
                bail!("moon.toml exceeds the 64 KiB size limit");
            }
            let source = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("moon.toml must contain UTF-8 text"))?;
            // TOML parser errors include source lines and unknown field names.
            // Do not propagate them: a misplaced credential must remain private.
            toml::from_str::<ConfigFile>(source).map_err(|_| {
                anyhow::anyhow!(
                    "moon.toml is invalid; check TOML syntax, field types and supported learning fields"
                )
            })?
        }
        None => ConfigFile::default(),
    };
    let base = path.parent().context("moon.toml parent is unavailable")?;
    config.learning.validate(base)?;
    Ok(ResolvedConfig {
        path,
        present,
        learning: config.learning,
    })
}

/// Validate external prompt files and the IANA timezone in addition to the
/// schema. No provider request is made and no prompt content is returned.
pub fn validate(home: &Path) -> Result<ResolvedConfig> {
    let config = load(home)?;
    if config
        .learning
        .l2
        .timezone
        .parse::<chrono_tz::Tz>()
        .is_err()
    {
        bail!("learning.l2.timezone is not a recognised IANA timezone");
    }
    validate_prompt(
        "learning.l1.prompt_file",
        config.learning.l1.prompt_file.as_deref(),
    )?;
    validate_prompt(
        "learning.l2.prompt_file",
        config.learning.l2.prompt_file.as_deref(),
    )?;
    Ok(config)
}

/// Atomically create an owner-only starter file without replacing existing data.
pub fn init(home: &Path) -> Result<ResolvedConfig> {
    let path = absolute_path(&home.join("moon.toml"))?;
    let parent = path.parent().context("moon.toml parent is unavailable")?;
    if fs::symlink_metadata(&path).is_ok() {
        bail!("moon.toml already exists; refusing to overwrite it");
    }
    fs::create_dir_all(parent).context("could not create the selected Moon home")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".moon-config-")
        .tempfile_in(parent)
        .context("could not stage moon.toml")?;
    staged
        .write_all(STARTER_CONFIG.as_bytes())
        .context("could not write moon.toml")?;
    staged
        .as_file()
        .sync_all()
        .context("could not sync moon.toml")?;
    staged.persist_noclobber(&path).map_err(|error| {
        if error.error.kind() == ErrorKind::AlreadyExists {
            anyhow::anyhow!("moon.toml already exists; refusing to overwrite it")
        } else {
            anyhow::anyhow!("could not create moon.toml")
        }
    })?;
    load(home)
}

impl LearningConfig {
    fn validate(&mut self, base: &Path) -> Result<()> {
        check_bounds(
            "learning.observation_ttl_hours",
            self.observation_ttl_hours.into(),
            1,
            8_760,
        )?;
        check_model("learning.l1.model", self.l1.model.as_deref())?;
        check_reasoning(
            "learning.l1.reasoning",
            self.l1.reasoning.as_deref(),
            self.l1.model.as_deref(),
        )?;
        check_model(
            "learning.l1.fallback_model",
            self.l1.fallback_model.as_deref(),
        )?;
        check_reasoning(
            "learning.l1.fallback_reasoning",
            self.l1.fallback_reasoning.as_deref(),
            self.l1.fallback_model.as_deref(),
        )?;
        check_optional_bounds(
            "learning.l1.timeout_ms",
            self.l1.timeout_ms,
            1_000,
            1_800_000,
        )?;
        check_optional_bounds(
            "learning.l1.max_output_tokens",
            self.l1.max_output_tokens.map(u64::from),
            128,
            131_072,
        )?;
        resolve_prompt("learning.l1.prompt_file", &mut self.l1.prompt_file, base)?;

        check_model("learning.l2.model", self.l2.model.as_deref())?;
        check_reasoning(
            "learning.l2.reasoning",
            self.l2.reasoning.as_deref(),
            self.l2.model.as_deref(),
        )?;
        check_model(
            "learning.l2.fallback_model",
            self.l2.fallback_model.as_deref(),
        )?;
        check_reasoning(
            "learning.l2.fallback_reasoning",
            self.l2.fallback_reasoning.as_deref(),
            self.l2.fallback_model.as_deref(),
        )?;
        check_optional_bounds(
            "learning.l2.timeout_ms",
            self.l2.timeout_ms,
            1_000,
            1_800_000,
        )?;
        check_optional_bounds(
            "learning.l2.max_output_tokens",
            self.l2.max_output_tokens.map(u64::from),
            128,
            131_072,
        )?;
        resolve_prompt("learning.l2.prompt_file", &mut self.l2.prompt_file, base)?;
        check_bounds("learning.l2.batch_size", self.l2.batch_size.into(), 1, 128)?;
        check_bounds(
            "learning.l2.max_batches_per_day",
            self.l2.max_batches_per_day.into(),
            1,
            64,
        )?;
        check_bounds("learning.l2.max_actions", self.l2.max_actions.into(), 1, 32)?;
        check_bounds(
            "learning.l2.max_input_chars",
            self.l2.max_input_chars.into(),
            1_024,
            2_097_152,
        )?;
        check_daily_at(&self.l2.daily_at)?;
        check_timezone(&self.l2.timezone)
    }
}

fn check_model(field: &'static str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    if crate::redaction::redact_text(value).count > 0 {
        bail!("{field} must not contain credentials; authentication belongs to OpenClaw");
    }
    let valid = value.split_once('/').is_some_and(|(provider, model)| {
        !provider.is_empty()
            && !model.is_empty()
            && provider
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte))
    });
    if value.len() > 256 || value.contains("://") || !valid {
        bail!("{field} must be a provider-qualified model reference of at most 256 bytes");
    }
    Ok(())
}

fn check_reasoning(field: &'static str, value: Option<&str>, model: Option<&str>) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    if !matches!(
        value,
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "adaptive" | "max" | "ultra"
    ) {
        bail!("{field} must be a supported reasoning effort; use low rather than light");
    }
    if model
        .and_then(|model| model.split_once('/'))
        .is_some_and(|(_, model)| model == "gpt-6-astra" || model.starts_with("gpt-6-astra-"))
        && !matches!(value, "low" | "medium" | "high" | "xhigh" | "max" | "ultra")
    {
        bail!(
            "{field} is incompatible with GPT-6-Astra; choose a supported effort from low through ultra"
        );
    }
    Ok(())
}

fn check_optional_bounds(
    field: &'static str,
    value: Option<u64>,
    min: u64,
    max: u64,
) -> Result<()> {
    if let Some(value) = value {
        check_bounds(field, value, min, max)?;
    }
    Ok(())
}

fn check_bounds(field: &'static str, value: u64, min: u64, max: u64) -> Result<()> {
    if !(min..=max).contains(&value) {
        bail!("{field} must be between {min} and {max}");
    }
    Ok(())
}

fn resolve_prompt(field: &'static str, prompt: &mut Option<PathBuf>, base: &Path) -> Result<()> {
    if let Some(path) = prompt {
        let valid = path.to_str().is_some_and(|value| {
            !value.trim().is_empty() && value.len() <= 4_096 && !value.chars().any(char::is_control)
        });
        if !valid {
            bail!(
                "{field} must be a nonempty path of at most 4096 bytes without control characters"
            );
        }
        *path = absolute_path(&base.join(&*path))?;
    }
    Ok(())
}

fn check_daily_at(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || ![bytes[0], bytes[1], bytes[3], bytes[4]]
            .iter()
            .all(u8::is_ascii_digit)
        || (bytes[0] - b'0') * 10 + bytes[1] - b'0' > 23
        || (bytes[3] - b'0') * 10 + bytes[4] - b'0' > 59
    {
        bail!("learning.l2.daily_at must use 24-hour HH:MM format");
    }
    Ok(())
}

fn check_timezone(value: &str) -> Result<()> {
    // Keep show usable for inspecting a syntactically valid but unrecognised
    // zone; validate and the scheduler check the identifier against IANA data.
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_+-".contains(&byte));
    if !valid {
        bail!("learning.l2.timezone must use IANA timezone syntax");
    }
    Ok(())
}

fn validate_prompt(field: &'static str, prompt: Option<&Path>) -> Result<()> {
    let Some(path) = prompt else { return Ok(()) };
    let file = open_nonblocking(path).map_err(|_| anyhow::anyhow!("{field} could not be read"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow::anyhow!("{field} could not be inspected"))?;
    if !metadata.is_file() {
        bail!("{field} must be a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("{field} could not be read"))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        bail!("{field} exceeds the 64 KiB size limit");
    }
    std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!("{field} must contain UTF-8 text"))?;
    Ok(())
}

fn open_nonblocking(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening an accidentally configured FIFO must not block validation.
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    // Preserve symlink/parent semantics; lexical removal of `..` could select a
    // different runtime when a path component is a symlink.
    std::path::absolute(path).context("could not resolve the configuration path")
}
