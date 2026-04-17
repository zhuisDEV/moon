use crate::moon::audit;
use crate::moon::openai_codex_auth;
use crate::moon::paths::MoonPaths;
use crate::moon::util::{now_epoch_secs, truncate_with_ellipsis};
use anyhow::{Context, Result};
use chrono::{Datelike, Local, TimeZone};
use fs2::FileExt;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct DistillInput {
    pub session_id: String,
    pub archive_path: String,
    pub archive_text: String,
    pub archive_epoch_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillOutput {
    pub provider: String,
    pub summary: String,
    pub summary_path: String,
    pub audit_log_path: String,
    pub created_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkedDistillOutput {
    pub provider: String,
    pub summary: String,
    pub summary_path: String,
    pub audit_log_path: String,
    pub created_at_epoch_secs: u64,
    pub chunk_count: usize,
    pub chunk_target_bytes: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct WisdomDistillInput {
    pub trigger: String,
    pub day_epoch_secs: Option<u64>,
    pub source_paths: Vec<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DistillAuditEvent {
    at_epoch_secs: u64,
    mode: String,
    trigger: String,
    source_path: String,
    target_path: String,
    input_hash: String,
    output_hash: String,
    provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionData {
    pub entries: Vec<ProjectionEntry>,
    pub tool_calls: Vec<String>,
    pub keywords: Vec<String>,
    pub topics: Vec<String>,
    pub time_start_epoch: Option<u64>,
    pub time_end_epoch: Option<u64>,
    pub message_count: usize,
    pub filtered_noise_count: usize,
    pub truncated: bool,
    pub compaction_anchors: Vec<CompactionAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionAnchor {
    pub note: String,
    pub origin_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolPriority {
    High,
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionEntry {
    pub timestamp_epoch: Option<u64>,
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_target: Option<String>,
    pub priority: Option<ToolPriority>,
    pub coupled_result: Option<String>,
}

pub trait Distiller {
    fn distill(&self, input: &DistillInput) -> Result<String>;
}

pub struct LocalDistiller;
pub struct GeminiDistiller {
    pub api_key: String,
    pub model: String,
}
pub struct OpenAiDistiller {
    pub api_key: String,
    pub model: String,
}
pub struct OpenAiCodexDistiller {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}
pub struct AnthropicDistiller {
    pub api_key: String,
    pub model: String,
}
pub struct OpenAiCompatDistiller {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteProvider {
    OpenAi,
    OpenAiCodex,
    Anthropic,
    Gemini,
    OpenAiCompatible,
}

impl RemoteProvider {
    fn label(self) -> &'static str {
        match self {
            RemoteProvider::OpenAi => "openai",
            RemoteProvider::OpenAiCodex => "openai-codex",
            RemoteProvider::Anthropic => "anthropic",
            RemoteProvider::Gemini => "gemini",
            RemoteProvider::OpenAiCompatible => "openai-compatible",
        }
    }
}

#[derive(Debug, Clone)]
struct RemoteModelConfig {
    provider: RemoteProvider,
    model: String,
    api_key: String,
    base_url: Option<String>,
}

const SIGNAL_KEYWORDS: [&str; 5] = ["decision", "rule", "todo", "next", "milestone"];
const MAX_SIGNAL_LINES: usize = 20;
const MAX_FALLBACK_LINES: usize = 12;
const MAX_CANDIDATE_CHARS: usize = 512;
const MAX_SUMMARY_CHARS: usize = 12_000;
const MAX_PROMPT_LINES: usize = 80;
const MAX_MODEL_LINES: usize = 80;
const MIN_MODEL_BULLETS: usize = 3;
const REQUEST_TIMEOUT_SECS: u64 = 45;
const DEFAULT_DISTILL_CHUNK_BYTES: usize = 512 * 1024;
const DEFAULT_DISTILL_MAX_CHUNKS: usize = 128;
const DEFAULT_AUTO_CONTEXT_TOKENS: u64 = 250_000;
const MIN_DISTILL_CHUNK_BYTES: usize = 64 * 1024;
const MAX_AUTO_CHUNK_BYTES: usize = 2 * 1024 * 1024;
const AUTO_CHUNK_BYTES_PER_TOKEN: f64 = 3.0;
const AUTO_CHUNK_SAFETY_RATIO: f64 = 0.60;
const MAX_ROLLUP_LINES_PER_SECTION: usize = 30;
const MAX_ROLLUP_TOTAL_LINES: usize = 120;
const MAX_ARCHIVE_SCAN_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARCHIVE_SCAN_LINES: usize = 200_000;
const MAX_ARCHIVE_CANDIDATES: usize = 2_000;
const MAX_WISDOM_LINES: usize = 240;
const MAX_WISDOM_ITEMS_PER_SECTION: usize = 8;
const WISDOM_CONTEXT_SAFETY_RATIO: f64 = 0.90;
const WISDOM_PROMPT_OVERHEAD_BYTES: usize = 8 * 1024;
const WISDOM_MIN_DAILY_CHUNK_BYTES: usize = 16 * 1024;
const DAILY_MEMORY_FORMAT_MARKER: &str = "<!-- moon_memory_format: conversation_v1 -->";
const SESSION_BLOCK_BEGIN_PREFIX: &str = "<!-- MOON_SESSION_BEGIN:";
const SESSION_BLOCK_END_PREFIX: &str = "<!-- MOON_SESSION_END:";
const L1_NORM_LOCK_FILE: &str = "l1-normalisation.lock";
const MEMORY_LOCK_FILE: &str = "memory.md.lock";
const DISTILL_AUDIT_FILE: &str = "distill.audit.log";
const ENTITY_ANCHORS_BEGIN: &str = "<!-- MOON_ENTITY_ANCHORS_BEGIN -->";
const ENTITY_ANCHORS_END: &str = "<!-- MOON_ENTITY_ANCHORS_END -->";
const TOPIC_STOPWORDS: [&str; 38] = [
    "the", "and", "for", "with", "that", "this", "from", "into", "about", "after", "before",
    "were", "was", "are", "is", "be", "been", "being", "have", "has", "had", "will", "would",
    "should", "could", "can", "did", "done", "not", "you", "your", "our", "their", "they", "them",
    "then", "than", "there",
];

static AUTO_CHUNK_BYTES_CACHE: OnceLock<usize> = OnceLock::new();

fn env_non_empty(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

fn parse_provider_alias(raw: &str) -> Option<RemoteProvider> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openai" => Some(RemoteProvider::OpenAi),
        "openai-codex" | "codex" => Some(RemoteProvider::OpenAiCodex),
        "anthropic" | "claude" => Some(RemoteProvider::Anthropic),
        "gemini" | "google" => Some(RemoteProvider::Gemini),
        "openai-compatible" | "compatible" | "deepseek" => Some(RemoteProvider::OpenAiCompatible),
        _ => None,
    }
}

fn parse_prefixed_model(raw: &str) -> (Option<RemoteProvider>, String) {
    let trimmed = raw.trim();
    if let Some((prefix, model)) = trimmed.split_once(':')
        && let Some(provider) = parse_provider_alias(prefix)
    {
        return (Some(provider), model.trim().to_string());
    }
    (None, trimmed.to_string())
}

fn infer_provider_from_model(model: &str) -> Option<RemoteProvider> {
    let lower = model.trim().to_ascii_lowercase();
    if lower.contains("codex") {
        return Some(RemoteProvider::OpenAiCodex);
    }
    if lower.starts_with("deepseek-") {
        return Some(RemoteProvider::OpenAiCompatible);
    }
    if lower.starts_with("claude-") {
        return Some(RemoteProvider::Anthropic);
    }
    if lower.starts_with("gemini-") {
        return Some(RemoteProvider::Gemini);
    }
    if lower.starts_with("gpt-")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        return Some(RemoteProvider::OpenAi);
    }
    None
}

fn first_available_provider() -> Option<RemoteProvider> {
    if env_non_empty("AI_BASE_URL").is_some() && env_non_empty("AI_API_KEY").is_some() {
        return Some(RemoteProvider::OpenAiCompatible);
    }
    if env_non_empty("AI_API_KEY").is_some() {
        return Some(RemoteProvider::OpenAiCompatible);
    }
    if openai_codex_auth::has_available_auth() {
        return Some(RemoteProvider::OpenAiCodex);
    }
    if env_non_empty("OPENAI_API_KEY").is_some() {
        return Some(RemoteProvider::OpenAi);
    }
    if env_non_empty("ANTHROPIC_API_KEY").is_some() {
        return Some(RemoteProvider::Anthropic);
    }
    if env_non_empty("GEMINI_API_KEY").is_some() {
        return Some(RemoteProvider::Gemini);
    }
    None
}

fn default_model_for_provider(provider: RemoteProvider) -> &'static str {
    match provider {
        RemoteProvider::OpenAi => "gpt-4.1-mini",
        RemoteProvider::OpenAiCodex => "gpt-5.4",
        RemoteProvider::Anthropic => "claude-3-5-haiku-latest",
        RemoteProvider::Gemini => "gemini-2.5-flash-lite",
        RemoteProvider::OpenAiCompatible => "deepseek-chat",
    }
}

fn resolve_api_key(provider: RemoteProvider) -> Result<Option<String>> {
    Ok(match provider {
        RemoteProvider::OpenAi => {
            env_non_empty("OPENAI_API_KEY").or_else(|| env_non_empty("AI_API_KEY"))
        }
        RemoteProvider::OpenAiCodex => openai_codex_auth::resolve_bearer_token()?,
        RemoteProvider::Anthropic => {
            env_non_empty("ANTHROPIC_API_KEY").or_else(|| env_non_empty("AI_API_KEY"))
        }
        RemoteProvider::Gemini => {
            env_non_empty("GEMINI_API_KEY").or_else(|| env_non_empty("AI_API_KEY"))
        }
        RemoteProvider::OpenAiCompatible => env_non_empty("AI_API_KEY")
            .or_else(|| env_non_empty("DEEPSEEK_API_KEY"))
            .or_else(|| env_non_empty("OPENAI_API_KEY")),
    })
}

fn resolve_compatible_base_url(model: &str) -> Option<String> {
    if let Some(base) = env_non_empty("AI_BASE_URL") {
        return Some(base);
    }
    if model.trim().to_ascii_lowercase().starts_with("deepseek-") {
        return Some("https://api.deepseek.com".to_string());
    }
    None
}

fn resolve_openai_codex_base_url() -> String {
    env_non_empty("OPENAI_CODEX_BASE_URL")
        .or_else(|| env_non_empty("OPENAI_BASE_URL"))
        .unwrap_or_else(|| "https://chatgpt.com/backend-api".to_string())
}

fn openai_codex_url(base_url: Option<&str>) -> String {
    let base = base_url
        .unwrap_or("https://chatgpt.com/backend-api")
        .trim_end_matches('/');
    if base.ends_with("/codex/responses") {
        base.to_string()
    } else {
        format!("{base}/codex/responses")
    }
}

fn call_openai_codex_prompt(
    client: &Client,
    base_url: Option<&str>,
    api_key: &str,
    model: &str,
    prompt: &str,
    stage: &str,
) -> Result<String> {
    let url = openai_codex_url(base_url);
    let payload = serde_json::json!({
        "model": model,
        "input": prompt,
        "store": false
    });
    let response = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&payload)
        .send()?;
    if !response.status().is_success() {
        anyhow::bail!(
            "openai-codex {} call failed with status {}",
            stage,
            response.status()
        );
    }
    let json: Value = response.json()?;
    extract_openai_text(&json)
        .with_context(|| format!("openai-codex {} response missing text content", stage))
}

fn resolve_remote_config() -> Option<RemoteModelConfig> {
    if env_non_empty("MOON_DISTILL_PROVIDER")
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("local"))
    {
        return None;
    }

    let configured_model = env_non_empty("MOON_DISTILL_MODEL")
        .or_else(|| env_non_empty("AI_MODEL"))
        .or_else(|| env_non_empty("MOON_GEMINI_MODEL"))
        .or_else(|| first_available_provider().map(|p| default_model_for_provider(p).to_string()));

    let mut chosen_provider = env_non_empty("MOON_DISTILL_PROVIDER")
        .as_deref()
        .and_then(parse_provider_alias)
        .or_else(|| {
            env_non_empty("AI_PROVIDER")
                .as_deref()
                .and_then(parse_provider_alias)
        });
    let (prefixed_provider, mut model) = configured_model
        .as_deref()
        .map(parse_prefixed_model)
        .unwrap_or((None, String::new()));
    if chosen_provider.is_none() {
        chosen_provider = prefixed_provider
            .or_else(|| infer_provider_from_model(&model))
            .or_else(first_available_provider);
    }

    let provider = chosen_provider?;
    if model.trim().is_empty() {
        model = default_model_for_provider(provider).to_string();
    }
    let base_url = match provider {
        RemoteProvider::OpenAiCodex => Some(resolve_openai_codex_base_url()),
        RemoteProvider::OpenAiCompatible => resolve_compatible_base_url(&model),
        _ => None,
    };
    let api_key = resolve_api_key(provider).ok()??;
    Some(RemoteModelConfig {
        provider,
        model,
        api_key,
        base_url,
    })
}

fn token_limit_to_bytes_with_ratio(tokens: u64, safety_ratio: f64) -> usize {
    let estimated = (tokens as f64) * AUTO_CHUNK_BYTES_PER_TOKEN * safety_ratio;
    (estimated as usize).clamp(MIN_DISTILL_CHUNK_BYTES, MAX_AUTO_CHUNK_BYTES)
}

fn token_limit_to_chunk_bytes(tokens: u64) -> usize {
    token_limit_to_bytes_with_ratio(tokens, AUTO_CHUNK_SAFETY_RATIO)
}

fn parse_env_u64(var: &str) -> Option<u64> {
    env_non_empty(var).and_then(|raw| raw.trim().parse::<u64>().ok())
}

fn find_u64_paths(root: &Value, paths: &[&[&str]]) -> Option<u64> {
    for path in paths {
        let mut cursor = root;
        let mut found = true;
        for part in *path {
            let Some(next) = cursor.get(*part) else {
                found = false;
                break;
            };
            cursor = next;
        }
        if found && let Some(value) = cursor.as_u64() {
            return Some(value);
        }
    }
    None
}

fn detect_gemini_input_token_limit(api_key: &str, model: &str) -> Option<u64> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}?key={}",
        model, api_key
    );
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .ok()?;
    let response = client.get(&url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: Value = response.json().ok()?;
    json.get("inputTokenLimit").and_then(Value::as_u64)
}

fn detect_openai_compatible_input_token_limit(
    api_key: &str,
    base_url: Option<&str>,
    model: &str,
) -> Option<u64> {
    let base = base_url
        .map(str::to_string)
        .or_else(|| resolve_compatible_base_url(model))
        .unwrap_or_else(|| "https://api.openai.com".to_string());
    let url = format!("{}/v1/models", base.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .ok()?;
    let response = client.get(&url).bearer_auth(api_key).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let json: Value = response.json().ok()?;
    let data = json.get("data").and_then(Value::as_array)?;
    let entry = data
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(model))?;

    find_u64_paths(
        entry,
        &[
            &["context_window"],
            &["max_context_length"],
            &["max_input_tokens"],
            &["input_token_limit"],
            &["inputTokenLimit"],
            &["context_length"],
            &["n_ctx"],
            &["capabilities", "context_window"],
            &["capabilities", "max_context_length"],
            &["capabilities", "max_input_tokens"],
            &["capabilities", "input_token_limit"],
        ],
    )
}

fn infer_context_tokens_from_model(provider: RemoteProvider, model: &str) -> u64 {
    let lower = model.to_ascii_lowercase();
    match provider {
        RemoteProvider::Gemini => {
            if lower.starts_with("gemini-2.5") {
                1_000_000
            } else {
                250_000
            }
        }
        RemoteProvider::OpenAi => {
            if lower.starts_with("gpt-4.1") {
                1_000_000
            } else if lower.starts_with("gpt-4o") {
                128_000
            } else {
                200_000
            }
        }
        RemoteProvider::OpenAiCodex => 1_050_000,
        RemoteProvider::Anthropic => 200_000,
        RemoteProvider::OpenAiCompatible => {
            if lower.starts_with("deepseek-") {
                128_000
            } else {
                200_000
            }
        }
    }
}

fn detect_context_tokens_from_remote(remote: &RemoteModelConfig) -> Option<u64> {
    match remote.provider {
        RemoteProvider::Gemini => detect_gemini_input_token_limit(&remote.api_key, &remote.model),
        RemoteProvider::OpenAiCompatible => detect_openai_compatible_input_token_limit(
            &remote.api_key,
            remote.base_url.as_deref(),
            &remote.model,
        ),
        RemoteProvider::OpenAi | RemoteProvider::OpenAiCodex | RemoteProvider::Anthropic => None,
    }
}

fn detect_auto_chunk_bytes() -> usize {
    if let Some(tokens) = parse_env_u64("MOON_DISTILL_MODEL_CONTEXT_TOKENS") {
        return token_limit_to_chunk_bytes(tokens);
    }
    if let Ok(cfg) = crate::moon::config::load_config()
        && let Some(tokens) = cfg.distill.model_context_tokens
    {
        return token_limit_to_chunk_bytes(tokens);
    }

    if let Some(remote) = resolve_remote_config() {
        if let Some(tokens) = detect_context_tokens_from_remote(&remote) {
            return token_limit_to_chunk_bytes(tokens);
        }
        return token_limit_to_chunk_bytes(infer_context_tokens_from_model(
            remote.provider,
            &remote.model,
        ));
    }

    token_limit_to_chunk_bytes(DEFAULT_AUTO_CONTEXT_TOKENS)
}

pub fn distill_chunk_bytes() -> usize {
    let auto = || *AUTO_CHUNK_BYTES_CACHE.get_or_init(detect_auto_chunk_bytes);
    match env::var("MOON_DISTILL_CHUNK_BYTES") {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return auto();
            }
            if trimmed.eq_ignore_ascii_case("auto") {
                return auto();
            }
            trimmed
                .parse::<usize>()
                .ok()
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_DISTILL_CHUNK_BYTES)
                .max(MIN_DISTILL_CHUNK_BYTES)
        }
        Err(_) => {
            if let Ok(cfg) = crate::moon::config::load_config()
                && let Some(raw) = cfg.distill.chunk_bytes
            {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
                    return auto();
                }
                return trimmed
                    .parse::<usize>()
                    .ok()
                    .filter(|v| *v > 0)
                    .unwrap_or(DEFAULT_DISTILL_CHUNK_BYTES)
                    .max(MIN_DISTILL_CHUNK_BYTES);
            }
            auto()
        }
    }
}

fn distill_max_chunks() -> usize {
    match env::var("MOON_DISTILL_MAX_CHUNKS") {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return DEFAULT_DISTILL_MAX_CHUNKS;
            }
            trimmed
                .parse::<usize>()
                .ok()
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_DISTILL_MAX_CHUNKS)
        }
        Err(_) => {
            if let Ok(cfg) = crate::moon::config::load_config()
                && let Some(max_chunks) = cfg.distill.max_chunks
            {
                return usize::try_from(max_chunks)
                    .ok()
                    .filter(|v| *v > 0)
                    .unwrap_or(DEFAULT_DISTILL_MAX_CHUNKS);
            }
            DEFAULT_DISTILL_MAX_CHUNKS
        }
    }
}

pub fn archive_file_size(path: &str) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat {path}"))?
        .len())
}

fn unescape_json_noise(input: &str) -> String {
    input
        .replace("\\\\\"", "\"")
        .replace("\\\\n", "\n")
        .replace("\\\\t", "\t")
        .replace("\\\\\\\\", "\\")
}

fn normalize_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_candidate_text(input: &str) -> Option<String> {
    let unescaped = unescape_json_noise(input);
    let normalized = normalize_text(&unescaped);
    if normalized.is_empty() {
        return None;
    }
    Some(truncate_with_ellipsis(&normalized, MAX_CANDIDATE_CHARS))
}

fn looks_like_json_blob(input: &str) -> bool {
    let trimmed = input.trim_start();
    trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.contains("\"type\":\"message\"")
        || trimmed.contains("\"message\":{\"role\"")
}

fn push_message_candidates(entry: &Value, out: &mut Vec<String>) {
    let Some(message) = entry.get("message") else {
        return;
    };
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return;
    };

    for part in content {
        if part.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Some(cleaned) = clean_candidate_text(text) else {
            continue;
        };

        let candidate = match role {
            "toolResult" => {
                // Tool payloads can be huge JSON blobs; only keep concise plain-text outputs.
                if cleaned.len() > 220
                    || looks_like_json_blob(&cleaned)
                    || cleaned.contains("<<<EXTERNAL_UNTRUSTED_CONTENT>>>")
                {
                    continue;
                }
                format!("[tool] {cleaned}")
            }
            "user" => format!("[user] {cleaned}"),
            "assistant" => format!("[assistant] {cleaned}"),
            _ => cleaned,
        };
        out.push(candidate);
        if out.len() >= 200 {
            return;
        }
    }
}

fn push_candidate_from_line(trimmed: &str, out: &mut Vec<String>) {
    if trimmed.is_empty() {
        return;
    }

    if let Ok(entry) = serde_json::from_str::<Value>(trimmed) {
        push_message_candidates(&entry, out);
        return;
    }

    if !looks_like_json_blob(trimmed)
        && let Some(cleaned) = clean_candidate_text(trimmed)
    {
        out.push(cleaned);
    }
}

fn extract_candidate_lines(raw: &str) -> Vec<String> {
    let mut out = Vec::new();

    for line in raw.lines() {
        push_candidate_from_line(line.trim(), &mut out);

        if out.len() >= 200 {
            break;
        }
    }

    out
}

fn normalize_epoch_units(raw: u64) -> u64 {
    if raw >= 100_000_000_000_000 {
        // microseconds -> seconds
        raw / 1_000_000
    } else if raw >= 100_000_000_000 {
        // milliseconds -> seconds
        raw / 1_000
    } else {
        raw
    }
}

fn parse_timestamp_value(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| {
                number
                    .as_i64()
                    .and_then(|v| if v >= 0 { Some(v as u64) } else { None })
            })
            .map(normalize_epoch_units),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Ok(numeric) = trimmed.parse::<u64>() {
                return Some(normalize_epoch_units(numeric));
            }
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(trimmed) {
                let secs = parsed.timestamp();
                if secs >= 0 {
                    return Some(secs as u64);
                }
            }
            None
        }
        _ => None,
    }
}

fn resolve_entry_timestamp_epoch(entry: &Value, message: &Value) -> Option<u64> {
    for candidate in [
        message.get("createdAt"),
        message.get("timestamp"),
        entry.get("timestamp_epoch"),
        entry.get("timestamp"),
        entry.get("createdAt"),
        entry.get("created_at"),
    ] {
        let Some(value) = candidate else {
            continue;
        };
        if let Some(epoch) = parse_timestamp_value(value) {
            return Some(epoch);
        }
    }
    None
}

fn is_useful_text_signal(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 4 {
        return false;
    }
    if trimmed.len() > 300 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
        || lower.starts_with('/')
        || lower.starts_with("~/")
        || lower.contains("/users/")
    {
        return false;
    }
    trimmed.chars().any(|c| c.is_ascii_alphabetic())
}

fn should_collect_tool_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "prompt"
            | "query"
            | "text"
            | "description"
            | "caption"
            | "instruction"
            | "instructions"
            | "keywords"
            | "title"
            | "style"
            | "task"
            | "negative_prompt"
    ) || lower.contains("prompt")
        || lower.contains("query")
        || lower.contains("caption")
}

fn extract_flag_value(raw: &str, flag: &str) -> Option<String> {
    let pos = raw.find(flag)?;
    let mut rest = raw.get(pos + flag.len()..)?.trim_start();
    if rest.is_empty() {
        return None;
    }

    if let Some(stripped) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for ch in stripped.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                break;
            }
            out.push(ch);
        }
        return Some(out);
    }

    if let Some(stripped) = rest.strip_prefix('\'') {
        let mut out = String::new();
        for ch in stripped.chars() {
            if ch == '\'' {
                break;
            }
            out.push(ch);
        }
        return Some(out);
    }

    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    rest = &rest[..end];
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn collect_tool_input_signals(value: &Value, out: &mut BTreeSet<String>, depth: usize) {
    if depth > 4 || out.len() >= 12 {
        return;
    }

    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if out.len() >= 12 {
                    break;
                }
                if should_collect_tool_key(key)
                    && let Some(raw) = child.as_str()
                    && is_useful_text_signal(raw)
                    && let Some(cleaned) = clean_candidate_text(raw)
                {
                    out.insert(cleaned);
                }
                if key.eq_ignore_ascii_case("command")
                    && let Some(raw) = child.as_str()
                {
                    for flag in ["--prompt", "--query"] {
                        if let Some(extracted) = extract_flag_value(raw, flag)
                            && is_useful_text_signal(&extracted)
                            && let Some(cleaned) = clean_candidate_text(&extracted)
                        {
                            out.insert(cleaned);
                        }
                    }
                }
                collect_tool_input_signals(child, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for child in items.iter().take(16) {
                if out.len() >= 12 {
                    break;
                }
                collect_tool_input_signals(child, out, depth + 1);
            }
        }
        Value::String(raw) => {
            if is_useful_text_signal(raw)
                && let Some(cleaned) = clean_candidate_text(raw)
            {
                out.insert(cleaned);
            }
        }
        _ => {}
    }
}

fn extract_message_entry(entry: &Value) -> Option<ProjectionEntry> {
    let message = entry.get("message")?;
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let timestamp_epoch = resolve_entry_timestamp_epoch(entry, message);

    let content_arr = message.get("content").and_then(Value::as_array)?;
    let mut text_parts = Vec::new();
    let mut tool_name = None;
    let mut tool_target = None;
    let mut priority = None;

    if role == "toolResult" {
        for part in content_arr {
            if part.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = part.get("text").and_then(Value::as_str)
                && let Some(cleaned) = clean_candidate_text(text)
                && cleaned.len() <= 1024
                && !looks_like_json_blob(&cleaned)
                && !cleaned.contains("<<<EXTERNAL_UNTRUSTED_CONTENT>>>")
            {
                text_parts.push(cleaned);
            }
        }
    } else {
        for part in content_arr {
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
            if part_type == "text" {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && let Some(cleaned) = clean_candidate_text(text)
                {
                    text_parts.push(cleaned);
                }
            } else if (part_type == "toolUse" || part_type == "toolCall")
                && let Some(name) = part.get("name").and_then(Value::as_str)
            {
                tool_name = Some(name.to_string());
                priority = Some(match name {
                    "write_to_file" | "exec" | "edit" | "gateway" => ToolPriority::High,
                    _ => ToolPriority::Normal,
                });

                if let Some(input) = part
                    .get("input")
                    .or_else(|| part.get("arguments"))
                    .and_then(Value::as_object)
                {
                    if let Some(cmd) = input.get("command").and_then(Value::as_str) {
                        tool_target = Some(cmd.to_string());
                    } else if let Some(path) = input
                        .get("path")
                        .or_else(|| input.get("file"))
                        .and_then(Value::as_str)
                    {
                        tool_target = Some(path.to_string());
                    } else if let Ok(dump) = serde_json::to_string(input) {
                        tool_target = Some(truncate_with_ellipsis(&dump, 64));
                    }
                }

                if let Some(input_value) = part.get("input").or_else(|| part.get("arguments")) {
                    let mut tool_signals = BTreeSet::new();
                    collect_tool_input_signals(input_value, &mut tool_signals, 0);
                    for signal in tool_signals {
                        text_parts.push(format!("[tool-input] {signal}"));
                    }
                }
            }
        }
    }

    if text_parts.is_empty() && tool_name.is_none() {
        return None;
    }

    Some(ProjectionEntry {
        timestamp_epoch,
        role,
        content: text_parts.join("\n"),
        tool_name,
        tool_target,
        priority,
        coupled_result: None,
    })
}

fn is_no_reply_marker(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("no_reply")
}

fn is_poll_heartbeat_noise(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("\"action\":\"poll\"")
        || lower.contains("\"action\": \"poll\"")
        || lower.contains("[tool-input] poll")
        || lower.contains("command still running (session")
        || lower.contains("(no new output) process still running")
        || lower.contains("use process (list/poll/log/write/kill/clear/remove)")
}

fn is_status_echo_noise(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("command: moon-watch") || lower.contains("command: watch"))
        && lower.contains("moon watcher cycle completed")
        && lower.contains("heartbeat_epoch_secs=")
        && lower.contains("poll_interval_secs=")
        || (lower.contains("threshold.trigger=")
            && lower.contains("distill.mode=")
            && lower.contains("embed.mode="))
}

fn is_projection_noise_entry(entry: &ProjectionEntry) -> bool {
    let combined = if entry.content.trim().is_empty() {
        entry.tool_target.as_deref().unwrap_or_default().to_string()
    } else if let Some(tool_target) = entry.tool_target.as_deref() {
        format!("{} {}", entry.content, tool_target)
    } else {
        entry.content.clone()
    };

    if combined.trim().is_empty() {
        return false;
    }

    if is_no_reply_marker(&combined) {
        return true;
    }
    if is_poll_heartbeat_noise(&combined) {
        return true;
    }
    if is_status_echo_noise(&combined) {
        return true;
    }

    if entry.role == "assistant"
        && entry
            .tool_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("process"))
        && is_poll_heartbeat_noise(&combined)
    {
        return true;
    }

    false
}

fn extract_keywords(entries: &[ProjectionEntry]) -> Vec<String> {
    let mut keywords = BTreeSet::new();
    for entry in entries {
        if entry.role != "user" && entry.role != "assistant" {
            continue;
        }
        for word in entry
            .content
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
        {
            if word.len() > 4 && word.len() < 24 && !word.chars().all(|c| c.is_numeric()) {
                keywords.insert(word.to_lowercase());
            }
        }
        if keywords.len() > 100 {
            break;
        }
    }
    keywords.into_iter().take(30).collect()
}

fn infer_topics(_entries: &[ProjectionEntry], keywords: &[String]) -> Vec<String> {
    if keywords.is_empty() {
        vec![]
    } else {
        vec!["Session activity".to_string()]
    }
}

pub fn extract_projection_data(path: &str) -> Result<ProjectionData> {
    let file = fs::File::open(path).with_context(|| format!("failed to open {path}"))?;
    let reader = BufReader::new(file);

    let mut scanned_bytes = 0usize;
    let mut scanned_lines = 0usize;
    let mut entries: Vec<ProjectionEntry> = Vec::new();
    let mut tool_calls_set = BTreeSet::new();
    let mut compaction_anchors = Vec::new();
    let mut filtered_noise_count = 0usize;
    let mut truncated = false;

    let mut pending_tool_uses: Vec<usize> = Vec::new();

    for line in reader.split(b'\n') {
        let raw = line.with_context(|| format!("failed to read line from {path}"))?;
        scanned_lines = scanned_lines.saturating_add(1);
        scanned_bytes = scanned_bytes.saturating_add(raw.len().saturating_add(1));

        let decoded = String::from_utf8_lossy(&raw);
        let trimmed = decoded.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(json_entry) = serde_json::from_str::<Value>(trimmed) {
            if let Some(note) = json_entry.get("compaction_summary").and_then(Value::as_str) {
                compaction_anchors.push(CompactionAnchor {
                    note: note.to_string(),
                    origin_message_id: json_entry
                        .get("message_id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                });
            }

            if let Some(entry) = extract_message_entry(&json_entry) {
                if is_projection_noise_entry(&entry) {
                    filtered_noise_count = filtered_noise_count.saturating_add(1);
                    if entry.role == "toolResult" {
                        let _ = pending_tool_uses.pop();
                    }
                    continue;
                }

                let idx = entries.len();

                if entry.role == "assistant" && entry.tool_name.is_some() {
                    tool_calls_set.insert(entry.tool_name.clone().unwrap());
                    pending_tool_uses.push(idx);
                } else if entry.role == "toolResult"
                    && let Some(use_idx) = pending_tool_uses.pop()
                {
                    entries[use_idx].coupled_result = Some(entry.content.clone());
                }

                entries.push(entry);
            }
        } else if !looks_like_json_blob(trimmed)
            && let Some(cleaned) = clean_candidate_text(trimmed)
        {
            let entry = ProjectionEntry {
                timestamp_epoch: None,
                role: "system".to_string(),
                content: cleaned,
                tool_name: None,
                tool_target: None,
                priority: None,
                coupled_result: None,
            };
            if is_projection_noise_entry(&entry) {
                filtered_noise_count = filtered_noise_count.saturating_add(1);
            } else {
                entries.push(entry);
            }
        }

        if entries.len() >= MAX_ARCHIVE_CANDIDATES
            || scanned_lines >= MAX_ARCHIVE_SCAN_LINES
            || scanned_bytes >= MAX_ARCHIVE_SCAN_BYTES
        {
            truncated = true;
            break;
        }
    }

    let message_count = entries.len();
    let time_start_epoch = entries
        .iter()
        .filter_map(|entry| entry.timestamp_epoch)
        .min();
    let time_end_epoch = entries
        .iter()
        .filter_map(|entry| entry.timestamp_epoch)
        .max();
    let keywords = extract_keywords(&entries);
    let topics = infer_topics(&entries, &keywords);

    Ok(ProjectionData {
        entries,
        tool_calls: tool_calls_set.into_iter().collect(),
        keywords,
        topics,
        time_start_epoch,
        time_end_epoch,
        message_count,
        filtered_noise_count,
        truncated,
        compaction_anchors,
    })
}

impl ProjectionData {
    pub fn to_excerpt(&self) -> String {
        let mut out = Vec::new();
        for entry in &self.entries {
            let candidate = match entry.role.as_str() {
                "toolResult" => {
                    if entry.coupled_result.is_none() {
                        format!("[tool] {}", entry.content)
                    } else {
                        continue;
                    }
                }
                "user" => format!("[user] {}", entry.content),
                "assistant" => {
                    let mut s = format!("[assistant] {}", entry.content);
                    if let Some(ref t) = entry.tool_name {
                        s.push_str(&format!(" [toolUse {}]", t));
                    }
                    if let Some(ref r) = entry.coupled_result {
                        s.push_str(&format!("\n[toolResult] {}", r));
                    }
                    s
                }
                _ => entry.content.clone(),
            };
            if !candidate.trim().is_empty() {
                out.push(candidate);
            }
        }
        let mut excerpt = out.join("\n");
        if self.truncated {
            excerpt.push_str("\n[source excerpt truncated]");
        }
        excerpt
    }
}

pub fn load_source_excerpt(path: &str) -> Result<String> {
    let data = extract_projection_data(path)?;
    Ok(data.to_excerpt())
}

fn is_signal_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    SIGNAL_KEYWORDS
        .iter()
        .any(|keyword| lower.contains(keyword))
}

fn extract_signal_lines(raw: &str) -> Vec<String> {
    let candidates = extract_candidate_lines(raw);
    let mut out = Vec::new();

    for line in &candidates {
        if is_signal_line(line) {
            out.push(line.clone());
        }
        if out.len() >= MAX_SIGNAL_LINES {
            return out;
        }
    }

    if out.is_empty() {
        candidates.into_iter().take(MAX_FALLBACK_LINES).collect()
    } else {
        out
    }
}

fn build_prompt_context(raw: &str) -> String {
    let candidates = extract_candidate_lines(raw);
    let mut out = String::new();
    for line in candidates.into_iter().take(MAX_PROMPT_LINES) {
        out.push_str("- ");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn build_llm_prompt(input: &DistillInput) -> String {
    let context = build_prompt_context(&input.archive_text);
    format!(
        "Summarize this session into concise bullets under headings for Decisions, Rules, Milestones, and Open Tasks. Return markdown only. Never output raw JSON, JSONL, code fences, XML, YAML, tool payload dumps, or verbatim logs.\nSession id: {}\nArchive path: {}\n\nContext lines:\n{}",
        input.session_id, input.archive_path, context
    )
}

fn looks_like_structured_fragment(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with("```")
        || trimmed == "{"
        || trimmed == "}"
        || trimmed == "["
        || trimmed == "]"
        || (trimmed.starts_with('"') && trimmed.contains("\":"))
}

fn extract_openai_text(json: &Value) -> Option<String> {
    if let Some(text) = json.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }

    let mut chunks = Vec::new();
    let output = json.get("output").and_then(Value::as_array)?;
    for item in output {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                chunks.push(text.to_string());
            }
        }
    }

    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n"))
    }
}

fn extract_anthropic_text(json: &Value) -> Option<String> {
    let mut chunks = Vec::new();
    let content = json.get("content").and_then(Value::as_array)?;
    for part in content {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            chunks.push(text.to_string());
        }
    }
    if chunks.is_empty() {
        None
    } else {
        Some(chunks.join("\n"))
    }
}

fn extract_openai_compatible_text(json: &Value) -> Option<String> {
    let choices = json.get("choices").and_then(Value::as_array)?;
    let first = choices.first()?;
    let content = first.get("message")?.get("content")?;
    match content {
        Value::String(s) => Some(s.to_string()),
        Value::Array(parts) => {
            let mut chunks = Vec::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    chunks.push(text.to_string());
                }
            }
            if chunks.is_empty() {
                None
            } else {
                Some(chunks.join("\n"))
            }
        }
        _ => None,
    }
}

fn sanitize_model_summary(summary: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut bullet_count = 0usize;

    for raw_line in summary.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if looks_like_json_blob(trimmed)
            || looks_like_structured_fragment(trimmed)
            || trimmed.contains("<<<EXTERNAL_UNTRUSTED_CONTENT>>>")
        {
            continue;
        }

        let cleaned = clean_candidate_text(trimmed)?;
        let normalized = if cleaned.starts_with('#') {
            cleaned
        } else if cleaned.starts_with("- ") {
            bullet_count += 1;
            cleaned
        } else if cleaned.starts_with("* ") {
            bullet_count += 1;
            cleaned.replacen("* ", "- ", 1)
        } else {
            bullet_count += 1;
            format!("- {cleaned}")
        };
        lines.push(normalized);
        if lines.len() >= MAX_MODEL_LINES {
            break;
        }
    }

    if bullet_count < MIN_MODEL_BULLETS {
        return None;
    }
    Some(lines.join("\n"))
}

fn clamp_summary(summary: &str) -> String {
    let normalized = summary.trim_end();
    if normalized.chars().count() <= MAX_SUMMARY_CHARS {
        return normalized.to_string();
    }
    let truncated = truncate_with_ellipsis(normalized, MAX_SUMMARY_CHARS);
    format!("{truncated}\n\n[summary truncated]")
}

impl Distiller for LocalDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let mut lines = extract_signal_lines(&input.archive_text);
        if lines.is_empty() {
            lines = input
                .archive_text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(MAX_FALLBACK_LINES)
                .filter_map(clean_candidate_text)
                .collect();
        }

        let mut summary = String::new();
        summary.push_str("## Distilled Session Summary\n");
        summary.push_str(&format!("- session_id: {}\n", input.session_id));
        summary.push_str(&format!("- archive_path: {}\n", input.archive_path));
        summary.push_str("- extracted_signals:\n");
        for line in lines {
            summary.push_str(&format!("  - {}\n", line));
        }
        Ok(summary)
    }
}

impl Distiller for GeminiDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let prompt = build_llm_prompt(input);

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        let payload = serde_json::json!({
            "contents": [
                {
                    "parts": [
                        {"text": prompt}
                    ]
                }
            ]
        });

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        let response = client.post(&url).json(&payload).send()?;
        if !response.status().is_success() {
            anyhow::bail!("gemini call failed with status {}", response.status());
        }
        let json: Value = response.json()?;
        let text = json
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("content"))
            .and_then(|v| v.get("parts"))
            .and_then(Value::as_array)
            .and_then(|parts| parts.first())
            .and_then(|v| v.get("text"))
            .and_then(Value::as_str)
            .context("gemini response missing text content")?;

        Ok(text.to_string())
    }
}

impl Distiller for OpenAiDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let prompt = build_llm_prompt(input);
        let payload = serde_json::json!({
            "model": self.model,
            "input": prompt,
            "temperature": 0.2
        });

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        let response = client
            .post("https://api.openai.com/v1/responses")
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!("openai call failed with status {}", response.status());
        }

        let json: Value = response.json()?;
        let text = extract_openai_text(&json).context("openai response missing text content")?;
        Ok(text)
    }
}

impl Distiller for OpenAiCodexDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let prompt = build_llm_prompt(input);
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        call_openai_codex_prompt(
            &client,
            Some(self.base_url.as_str()),
            &self.api_key,
            &self.model,
            &prompt,
            "distill",
        )
    }
}

impl Distiller for OpenAiCompatDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let prompt = build_llm_prompt(input);
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/chat/completions");
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.2
        });

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        let response = client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!(
                "openai-compatible call failed with status {}",
                response.status()
            );
        }

        let json: Value = response.json()?;
        let text = extract_openai_compatible_text(&json)
            .context("openai-compatible response missing text content")?;
        Ok(text)
    }
}

impl Distiller for AnthropicDistiller {
    fn distill(&self, input: &DistillInput) -> Result<String> {
        let prompt = build_llm_prompt(input);
        let payload = serde_json::json!({
            "model": self.model,
            "max_tokens": 1200,
            "temperature": 0.2,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload)
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!("anthropic call failed with status {}", response.status());
        }

        let json: Value = response.json()?;
        let text =
            extract_anthropic_text(&json).context("anthropic response missing text content")?;
        Ok(text)
    }
}

fn daily_memory_path(paths: &MoonPaths, archive_epoch_secs: Option<u64>) -> String {
    let timestamp = archive_epoch_secs
        .and_then(|secs| Local.timestamp_opt(secs as i64, 0).single())
        .unwrap_or_else(Local::now);
    let date = format!(
        "{:04}-{:02}-{:02}",
        timestamp.year(),
        timestamp.month(),
        timestamp.day()
    );
    paths
        .memory_dir
        .join(format!("{}.md", date))
        .display()
        .to_string()
}

fn distill_summary(input: &DistillInput) -> Result<(String, String)> {
    let mut local_summary_cache: Option<String> = None;
    let mut local_summary = || -> Result<String> {
        if let Some(existing) = &local_summary_cache {
            return Ok(existing.clone());
        }
        let summary = LocalDistiller.distill(input)?;
        local_summary_cache = Some(summary.clone());
        Ok(summary)
    };

    let (provider_used, generated_summary) = if let Some(remote) = resolve_remote_config() {
        let remote_result = match remote.provider {
            RemoteProvider::OpenAi => OpenAiDistiller {
                api_key: remote.api_key.clone(),
                model: remote.model.clone(),
            }
            .distill(input),
            RemoteProvider::OpenAiCodex => OpenAiCodexDistiller {
                api_key: remote.api_key.clone(),
                model: remote.model.clone(),
                base_url: remote
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "https://chatgpt.com/backend-api".to_string()),
            }
            .distill(input),
            RemoteProvider::Anthropic => AnthropicDistiller {
                api_key: remote.api_key.clone(),
                model: remote.model.clone(),
            }
            .distill(input),
            RemoteProvider::Gemini => GeminiDistiller {
                api_key: remote.api_key.clone(),
                model: remote.model.clone(),
            }
            .distill(input),
            RemoteProvider::OpenAiCompatible => OpenAiCompatDistiller {
                api_key: remote.api_key.clone(),
                model: remote.model.clone(),
                base_url: remote
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com".to_string()),
            }
            .distill(input),
        };

        match remote_result {
            Ok(out) => match sanitize_model_summary(&out) {
                Some(cleaned) => (remote.provider.label().to_string(), cleaned),
                None => ("local".to_string(), local_summary()?),
            },
            Err(_) => ("local".to_string(), local_summary()?),
        }
    } else {
        ("local".to_string(), local_summary()?)
    };
    let deduped = apply_semantic_dedup(&generated_summary);
    Ok((provider_used, clamp_summary(&deduped)))
}

fn topic_discovery_enabled() -> bool {
    if let Ok(cfg) = crate::moon::config::load_config() {
        return cfg.distill.topic_discovery;
    }
    match env::var("MOON_TOPIC_DISCOVERY") {
        Ok(raw) => matches!(raw.trim(), "1" | "true" | "TRUE" | "yes" | "on"),
        Err(_) => false,
    }
}

fn is_valid_topic_key(key: &str) -> bool {
    if key.len() < 3 || key.len() > 32 {
        return false;
    }
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let digit_count = key.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count > 2 {
        return false;
    }
    !matches!(
        key,
        "session"
            | "sessions"
            | "summary"
            | "decision"
            | "decisions"
            | "rules"
            | "rule"
            | "milestone"
            | "milestones"
            | "tasks"
            | "task"
            | "archive"
            | "archive_path"
            | "archive_jsonl_path"
            | "content_hash"
            | "time_range_utc"
            | "time_range_local"
            | "local_timezone"
    )
}

fn normalize_semantic_key_fragment(fragment: &str) -> Option<String> {
    let mut tokens = Vec::new();
    for token in fragment
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .map(|t| t.trim().to_ascii_lowercase())
    {
        if token.len() < 2 || token.len() > 32 {
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if TOPIC_STOPWORDS.contains(&token.as_str()) {
            continue;
        }
        tokens.push(token);
        if tokens.len() >= 6 {
            break;
        }
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join("_"))
    }
}

fn semantic_key_for_bullet(section: &str, bullet: &str) -> Option<String> {
    let cleaned = bullet
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim();
    if cleaned.is_empty() {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();

    if let Some((lhs, _)) = lower.split_once(':')
        && lhs.split_whitespace().count() <= 8
        && let Some(key) = normalize_semantic_key_fragment(lhs)
    {
        return Some(format!("{section}|{key}"));
    }
    if let Some((lhs, _)) = lower.split_once('=')
        && lhs.split_whitespace().count() <= 8
        && let Some(key) = normalize_semantic_key_fragment(lhs)
    {
        return Some(format!("{section}|{key}"));
    }

    for prefix in [
        "set ",
        "updated ",
        "update ",
        "switched ",
        "switch ",
        "enabled ",
        "enable ",
        "disabled ",
        "disable ",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let lhs = rest
                .split(" to ")
                .next()
                .unwrap_or(rest)
                .split(" for ")
                .next()
                .unwrap_or(rest);
            if let Some(key) = normalize_semantic_key_fragment(lhs) {
                return Some(format!("{section}|{key}"));
            }
        }
    }

    None
}

fn apply_semantic_dedup(summary: &str) -> String {
    let mut section = "root".to_string();
    let lines = summary
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let mut last_index_for_key = BTreeMap::<String, usize>::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            section = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
            continue;
        }
        if !(trimmed.starts_with("- ") || trimmed.starts_with("* ")) {
            continue;
        }
        if let Some(key) = semantic_key_for_bullet(&section, trimmed) {
            last_index_for_key.insert(key, idx);
        }
    }

    let mut out = Vec::with_capacity(lines.len());
    section.clear();
    section.push_str("root");
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            section = trimmed.trim_start_matches('#').trim().to_ascii_lowercase();
            out.push(line.clone());
            continue;
        }
        if !(trimmed.starts_with("- ") || trimmed.starts_with("* ")) {
            out.push(line.clone());
            continue;
        }
        if let Some(key) = semantic_key_for_bullet(&section, trimmed)
            && let Some(last_idx) = last_index_for_key.get(&key)
            && *last_idx != idx
        {
            continue;
        }
        out.push(line.clone());
    }

    out.join("\n")
}

fn discover_topic_tags(summary: &str) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();

    for token in summary.split_whitespace() {
        let trimmed = token
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '#' && c != '_' && c != '-');
        if let Some(tag_body) = trimmed.strip_prefix('#') {
            let lower = tag_body.to_ascii_lowercase();
            if let Some(key) = normalize_semantic_key_fragment(&lower)
                && is_valid_topic_key(&key)
            {
                *counts.entry(key).or_insert(0) += 3;
            }
            continue;
        }

        let lower = trimmed.to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        if matches!(
            lower.as_str(),
            "session"
                | "summary"
                | "decision"
                | "decisions"
                | "rule"
                | "rules"
                | "milestone"
                | "milestones"
                | "task"
                | "tasks"
                | "archive"
                | "path"
                | "distilled"
        ) {
            continue;
        }
        if let Some(key) = normalize_semantic_key_fragment(&lower)
            && is_valid_topic_key(&key)
        {
            *counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(8)
        .map(|(topic, _)| format!("#{topic}"))
        .collect()
}

fn build_entity_anchor_line(
    session_id: &str,
    archive_path: &str,
    tags: &[String],
) -> Option<String> {
    if tags.is_empty() {
        return None;
    }
    Some(format!(
        "- session_id={} archive_path={} topics={}",
        session_id,
        archive_path,
        tags.join(" ")
    ))
}

fn upsert_entity_anchors_block(
    existing: &str,
    session_id: &str,
    archive_path: &str,
    tags: &[String],
) -> String {
    let Some(new_line) = build_entity_anchor_line(session_id, archive_path, tags) else {
        return existing.to_string();
    };

    let mut anchor_lines = Vec::<String>::new();
    let mut body = existing.to_string();

    if let Some(start) = existing.find(ENTITY_ANCHORS_BEGIN)
        && let Some(end_rel) = existing[start..].find(ENTITY_ANCHORS_END)
    {
        let end = start + end_rel + ENTITY_ANCHORS_END.len();
        let block = &existing[start..end];
        for line in block.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("- ") {
                continue;
            }
            if trimmed.contains(&format!("session_id={}", session_id)) {
                continue;
            }
            anchor_lines.push(trimmed.to_string());
        }
        body = format!("{}{}", &existing[..start], &existing[end..]);
    }

    anchor_lines.push(new_line);
    anchor_lines.sort();

    let mut block = String::new();
    block.push_str(ENTITY_ANCHORS_BEGIN);
    block.push('\n');
    block.push_str("## Entity Anchors\n");
    for line in anchor_lines {
        block.push_str(&line);
        block.push('\n');
    }
    block.push_str(ENTITY_ANCHORS_END);
    block.push('\n');
    block.push('\n');

    format!("{}{}", block, body.trim_start())
}

fn append_distilled_summary(
    paths: &MoonPaths,
    input: &DistillInput,
    provider_used: String,
    summary: String,
) -> Result<DistillOutput> {
    let summary_path = daily_memory_path(paths, input.archive_epoch_secs);
    let mut full_text = fs::read_to_string(&summary_path).unwrap_or_default();
    let topic_tags = if topic_discovery_enabled() {
        discover_topic_tags(&summary)
    } else {
        Vec::new()
    };
    if !topic_tags.is_empty() {
        full_text = upsert_entity_anchors_block(
            &full_text,
            &input.session_id,
            &input.archive_path,
            &topic_tags,
        );
    }

    if !full_text.is_empty() && !full_text.ends_with('\n') {
        full_text.push('\n');
    }
    full_text.push_str(&format!("\n### {}\n", input.session_id));
    full_text.push_str(&summary);
    full_text.push('\n');

    fs::write(&summary_path, full_text)
        .with_context(|| format!("failed to write {}", summary_path))?;

    audit::append_event(
        paths,
        "distill",
        "ok",
        &format!(
            "distilled session {} into {} provider={} topic_count={}",
            input.session_id,
            summary_path,
            provider_used,
            topic_tags.len()
        ),
    )?;

    Ok(DistillOutput {
        provider: provider_used,
        summary,
        summary_path: summary_path.clone(),
        audit_log_path: paths.logs_dir.join("audit.log").display().to_string(),
        created_at_epoch_secs: now_epoch_secs()?,
    })
}

#[derive(Default)]
struct ChunkSummaryRollup {
    seen: BTreeSet<String>,
    decisions: Vec<String>,
    rules: Vec<String>,
    milestones: Vec<String>,
    tasks: Vec<String>,
    other: Vec<String>,
}

impl ChunkSummaryRollup {
    fn total_lines(&self) -> usize {
        self.decisions.len()
            + self.rules.len()
            + self.milestones.len()
            + self.tasks.len()
            + self.other.len()
    }

    fn push_line(&mut self, raw_line: &str) {
        if self.total_lines() >= MAX_ROLLUP_TOTAL_LINES {
            return;
        }

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            return;
        }

        let normalized = trimmed
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim();
        if normalized.is_empty() || normalized.starts_with('#') {
            return;
        }
        if looks_like_json_blob(normalized) || looks_like_structured_fragment(normalized) {
            return;
        }

        let Some(cleaned) = clean_candidate_text(normalized) else {
            return;
        };
        let key = cleaned.to_ascii_lowercase();
        if !self.seen.insert(key) {
            return;
        }

        let lower = cleaned.to_ascii_lowercase();
        let target = if lower.contains("decision") {
            &mut self.decisions
        } else if lower.contains("rule") {
            &mut self.rules
        } else if lower.contains("milestone") {
            &mut self.milestones
        } else if lower.contains("todo")
            || lower.contains("open task")
            || lower.contains("next")
            || lower.contains("follow up")
            || lower.contains("follow-up")
            || lower.contains("action item")
        {
            &mut self.tasks
        } else {
            &mut self.other
        };

        if target.len() < MAX_ROLLUP_LINES_PER_SECTION {
            target.push(cleaned);
        }
    }

    fn ingest_summary(&mut self, summary: &str) {
        for line in summary.lines() {
            self.push_line(line);
            if self.total_lines() >= MAX_ROLLUP_TOTAL_LINES {
                break;
            }
        }
    }

    fn render(
        &self,
        session_id: &str,
        archive_path: &str,
        chunk_count: usize,
        chunk_target_bytes: usize,
        max_chunks: usize,
        truncated: bool,
    ) -> String {
        fn append_section(out: &mut String, title: &str, lines: &[String]) {
            if lines.is_empty() {
                return;
            }
            out.push_str(&format!("### {title}\n"));
            for line in lines {
                out.push_str("- ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }

        let mut out = String::new();
        out.push_str("## Distilled Session Summary\n");
        out.push_str(&format!("- session_id: {session_id}\n"));
        out.push_str(&format!("- archive_path: {archive_path}\n"));
        out.push_str(&format!("- chunk_count: {chunk_count}\n"));
        out.push_str(&format!("- chunk_target_bytes: {chunk_target_bytes}\n"));
        if truncated {
            out.push_str(&format!(
                "- chunking_truncated: true (max_chunks={max_chunks})\n"
            ));
        }
        out.push('\n');

        append_section(&mut out, "Decisions", &self.decisions);
        append_section(&mut out, "Rules", &self.rules);
        append_section(&mut out, "Milestones", &self.milestones);
        append_section(&mut out, "Open Tasks", &self.tasks);
        append_section(&mut out, "Other Signals", &self.other);

        if self.total_lines() == 0 {
            out.push_str("### Notes\n- no high-signal lines extracted from chunk summaries\n");
        }

        out
    }
}

fn summarize_provider_mix(provider_counts: &BTreeMap<String, usize>) -> String {
    if provider_counts.is_empty() {
        return "local".to_string();
    }
    if provider_counts.len() == 1 {
        return provider_counts.keys().next().cloned().unwrap_or_default();
    }
    let parts = provider_counts
        .iter()
        .map(|(provider, count)| format!("{provider}:{count}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("mixed({parts})")
}

fn stream_archive_chunks<F>(
    path: &str,
    chunk_target_bytes: usize,
    max_chunks: usize,
    mut on_chunk: F,
) -> Result<(usize, bool)>
where
    F: FnMut(usize, String) -> Result<()>,
{
    let file = fs::File::open(path).with_context(|| format!("failed to open {path}"))?;
    let reader = BufReader::new(file);

    let mut current_chunk = String::new();
    let mut current_bytes = 0usize;
    let mut chunk_count = 0usize;
    let mut truncated = false;

    for line in reader.split(b'\n') {
        let raw = line.with_context(|| format!("failed to read line from {path}"))?;
        let line_bytes = raw.len().saturating_add(1);

        if !current_chunk.is_empty()
            && current_bytes.saturating_add(line_bytes) > chunk_target_bytes
        {
            chunk_count = chunk_count.saturating_add(1);
            on_chunk(chunk_count, std::mem::take(&mut current_chunk))?;
            current_bytes = 0;
            if chunk_count >= max_chunks {
                truncated = true;
                break;
            }
        }

        current_chunk.push_str(&String::from_utf8_lossy(&raw));
        current_chunk.push('\n');
        current_bytes = current_bytes.saturating_add(line_bytes);
    }

    if !truncated {
        if current_chunk.is_empty() {
            if chunk_count == 0 {
                chunk_count = 1;
                on_chunk(chunk_count, String::new())?;
            }
        } else {
            chunk_count = chunk_count.saturating_add(1);
            on_chunk(chunk_count, current_chunk)?;
        }
    }

    Ok((chunk_count, truncated))
}

pub fn run_chunked_archive_distillation(
    paths: &MoonPaths,
    input: &DistillInput,
) -> Result<ChunkedDistillOutput> {
    // Layer 1 is conversation-preserving normalization. Chunked mode is retained as a
    // compatibility wrapper and delegates to single-pass output generation.
    let out = run_distillation(paths, input)?;
    Ok(ChunkedDistillOutput {
        provider: out.provider.clone(),
        summary: out.summary.clone(),
        summary_path: out.summary_path,
        audit_log_path: out.audit_log_path,
        created_at_epoch_secs: out.created_at_epoch_secs,
        chunk_count: 1,
        chunk_target_bytes: distill_chunk_bytes(),
        truncated: false,
    })
}

fn session_block_markers(session_id: &str) -> (String, String) {
    (
        format!("{SESSION_BLOCK_BEGIN_PREFIX}{session_id} -->"),
        format!("{SESSION_BLOCK_END_PREFIX}{session_id} -->"),
    )
}

fn normalize_turn_text(raw: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("[tool-input]") || trimmed.starts_with("[toolResult]") {
            continue;
        }
        if is_poll_heartbeat_noise(trimmed) || is_status_echo_noise(trimmed) {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn strip_projection_bullet_text(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end_idx) = rest.find(']')
    {
        return rest[end_idx + 1..].trim().to_string();
    }
    trimmed.to_string()
}

type Layer1ProjectionExtract = (Vec<(String, String)>, Option<Vec<String>>, usize, usize);

fn extract_layer1_from_projection_markdown(projection_md: &str) -> Layer1ProjectionExtract {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        None,
        User,
        Assistant,
        Tool,
    }

    let mut section = Section::None;
    let mut turns = Vec::<(String, String)>::new();
    let mut tool_lines = Vec::<String>::new();
    let mut message_count: Option<usize> = None;
    let mut filtered_noise_count: Option<usize> = None;

    for raw_line in projection_md.lines() {
        let line = raw_line.trim();
        if let Some(raw_count) = line.strip_prefix("message_count:")
            && message_count.is_none()
        {
            message_count = raw_count.trim().parse::<usize>().ok();
        }
        if let Some(raw_count) = line.strip_prefix("filtered_noise_count:")
            && filtered_noise_count.is_none()
        {
            filtered_noise_count = raw_count.trim().parse::<usize>().ok();
        }

        if line.starts_with("### User Queries") {
            section = Section::User;
            continue;
        }
        if line.starts_with("### Assistant Responses") {
            section = Section::Assistant;
            continue;
        }
        if line.starts_with("## Tool Activity") {
            section = Section::Tool;
            continue;
        }
        if line.starts_with("## ")
            && !line.starts_with("## Tool Activity")
            && !line.starts_with("## Conversations")
        {
            section = Section::None;
            continue;
        }

        let Some(raw_bullet) = line.strip_prefix("- ") else {
            continue;
        };
        if raw_bullet.eq_ignore_ascii_case("none") {
            continue;
        }

        let content = strip_projection_bullet_text(raw_bullet);
        if content.is_empty() {
            continue;
        }

        match section {
            Section::User => {
                if let Some(cleaned) = normalize_turn_text(&content) {
                    turns.push(("user".to_string(), cleaned));
                }
            }
            Section::Assistant => {
                if let Some(cleaned) = normalize_turn_text(&content) {
                    turns.push(("assistant".to_string(), cleaned));
                }
            }
            Section::Tool => {
                if let Some(cleaned) = clean_candidate_text(&content) {
                    tool_lines.push(cleaned);
                }
            }
            Section::None => {}
        }
    }

    let execution_summary = build_execution_summary_from_turns_and_tools(&turns, &tool_lines);
    let fallback_messages = turns.len().saturating_add(tool_lines.len());
    (
        turns,
        execution_summary,
        message_count.unwrap_or(fallback_messages),
        filtered_noise_count.unwrap_or(0),
    )
}

fn find_notable_blocker(data: &ProjectionData) -> Option<String> {
    let keywords = ["error", "failed", "retry", "timeout", "blocked", "denied"];
    for entry in data.entries.iter().rev() {
        for candidate in [
            Some(entry.content.as_str()),
            entry.coupled_result.as_deref(),
        ] {
            let Some(text) = candidate else {
                continue;
            };
            let lower = text.to_ascii_lowercase();
            if keywords.iter().any(|kw| lower.contains(kw))
                && let Some(cleaned) = clean_candidate_text(text)
            {
                return Some(cleaned);
            }
        }
    }
    None
}

fn build_execution_summary_from_turns_and_tools(
    turns: &[(String, String)],
    tool_lines: &[String],
) -> Option<Vec<String>> {
    if tool_lines.is_empty() {
        return None;
    }

    let goal = turns
        .iter()
        .find(|(role, _)| role == "user")
        .map(|(_, text)| truncate_with_ellipsis(text, 220))
        .unwrap_or_else(|| "Complete the requested task from conversation context.".to_string());

    let mut seen = BTreeSet::new();
    let mut actions = Vec::new();
    for line in tool_lines {
        let action = truncate_with_ellipsis(line, 140);
        if seen.insert(action.to_ascii_lowercase()) {
            actions.push(action);
        }
        if actions.len() >= 4 {
            break;
        }
    }
    if actions.is_empty() {
        return None;
    }

    let outcome = turns
        .iter()
        .rev()
        .find(|(role, _)| role == "assistant")
        .map(|(_, text)| truncate_with_ellipsis(text, 220))
        .unwrap_or_else(|| "Task progressed based on the available execution steps.".to_string());

    let mut lines = vec![
        format!("- Goal: {goal}"),
        format!("- Key actions: {}", actions.join("; ")),
        format!("- Outcome: {outcome}"),
    ];

    let keywords = ["error", "failed", "retry", "timeout", "blocked", "denied"];
    if let Some(blocker) = tool_lines
        .iter()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            keywords.iter().any(|kw| lower.contains(kw))
        })
        .map(|line| truncate_with_ellipsis(line, 220))
    {
        lines.push(format!("- Notable blocker/retry: {blocker}"));
    }

    Some(lines)
}

fn build_execution_summary_lines(data: &ProjectionData) -> Option<Vec<String>> {
    let goal = data
        .entries
        .iter()
        .find(|entry| entry.role == "user")
        .and_then(|entry| normalize_turn_text(&entry.content))
        .map(|text| truncate_with_ellipsis(&text, 220))
        .unwrap_or_else(|| "Clarify and complete the requested task.".to_string());

    let mut actions = Vec::new();
    let mut seen_actions = BTreeSet::new();
    for entry in &data.entries {
        if entry.role != "assistant" {
            continue;
        }
        let Some(tool_name) = entry.tool_name.as_deref() else {
            continue;
        };
        let action = if let Some(target) = entry.tool_target.as_deref() {
            let trimmed = target.trim();
            if trimmed.is_empty() {
                format!("used `{tool_name}`")
            } else {
                format!(
                    "used `{tool_name}` on {}",
                    truncate_with_ellipsis(trimmed, 120)
                )
            }
        } else {
            format!("used `{tool_name}`")
        };
        if seen_actions.insert(action.clone()) {
            actions.push(action);
        }
        if actions.len() >= 4 {
            break;
        }
    }
    if actions.is_empty() {
        return None;
    }

    let outcome = data
        .entries
        .iter()
        .rev()
        .find(|entry| entry.role == "assistant")
        .and_then(|entry| normalize_turn_text(&entry.content))
        .map(|text| truncate_with_ellipsis(&text, 220))
        .unwrap_or_else(|| "Task progressed based on the conversation state.".to_string());

    let mut lines = vec![
        format!("- Goal: {goal}"),
        format!("- Key actions: {}", actions.join("; ")),
        format!("- Outcome: {outcome}"),
    ];
    if let Some(blocker) = find_notable_blocker(data) {
        lines.push(format!(
            "- Notable blocker/retry: {}",
            truncate_with_ellipsis(&blocker, 220)
        ));
    }
    Some(lines)
}

fn build_layer1_signal_summary(
    session_id: &str,
    archive_path: &str,
    turns: &[(String, String)],
    execution_summary: Option<&[String]>,
) -> String {
    let mut out = String::new();
    out.push_str("## L1 Normalisation Session Digest\n");
    out.push_str(&format!("- session_id: {session_id}\n"));
    out.push_str(&format!("- archive_path: {archive_path}\n"));
    if let Some(lines) = execution_summary {
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
    } else {
        out.push_str("- Key actions: none captured\n");
        let highlights = turns
            .iter()
            .filter(|(role, _)| role == "user")
            .take(3)
            .map(|(_, text)| format!("- User signal: {}", truncate_with_ellipsis(text, 180)))
            .collect::<Vec<_>>();
        if highlights.is_empty() {
            out.push_str("- Outcome: no user/assistant turns captured\n");
        } else {
            for line in highlights {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

fn render_layer1_session_block(
    input: &DistillInput,
    message_count: usize,
    filtered_noise_count: usize,
    turns: &[(String, String)],
    execution_summary: Option<&[String]>,
) -> String {
    let (begin_marker, end_marker) = session_block_markers(&input.session_id);
    let mut out = String::new();
    out.push_str(&begin_marker);
    out.push('\n');
    out.push_str(&format!("## Session {}\n", input.session_id));
    out.push_str(&format!("- Source Archive: `{}`\n", input.archive_path));
    out.push_str(&format!("- Message Count: {message_count}\n"));
    out.push_str(&format!("- Noise Filtered: {filtered_noise_count}\n\n"));
    out.push_str("### Conversation\n");
    if turns.is_empty() {
        out.push_str("- No user/assistant turns captured.\n");
    } else {
        for (role, text) in turns {
            let role_label = if role == "user" { "User" } else { "Assistant" };
            out.push_str(&format!("**{role_label}:** "));
            let mut lines = text.lines();
            if let Some(first) = lines.next() {
                out.push_str(first.trim());
                out.push('\n');
            } else {
                out.push('\n');
            }
            for line in lines {
                out.push_str(line.trim());
                out.push('\n');
            }
            out.push('\n');
        }
    }
    if let Some(summary_lines) = execution_summary {
        out.push_str("### Execution Summary\n");
        for line in summary_lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&end_marker);
    out.push('\n');
    out
}

fn upsert_marked_block(
    existing: &str,
    begin_marker: &str,
    end_marker: &str,
    block: &str,
) -> String {
    if let Some(start) = existing.find(begin_marker)
        && let Some(end_rel) = existing[start..].find(end_marker)
    {
        let end = start + end_rel + end_marker.len();
        let mut out = String::new();
        out.push_str(&existing[..start]);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block.trim_end());
        out.push('\n');
        let tail = existing[end..].trim_start_matches('\n');
        if !tail.is_empty() {
            out.push('\n');
            out.push_str(tail);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        return out;
    }

    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block.trim_end());
    out.push('\n');
    out
}

fn ensure_daily_memory_header(existing: &str, date_label: &str) -> String {
    if !existing.trim().is_empty() {
        return existing.to_string();
    }
    format!("# Daily Memory {date_label}\n{DAILY_MEMORY_FORMAT_MARKER}\n\n")
}

fn today_daily_memory_path(paths: &MoonPaths, epoch_secs: u64) -> String {
    daily_memory_path(paths, Some(epoch_secs))
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn atomic_write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("memory.md");
    let tmp_name = format!(".{file_name}.{}.tmp", std::process::id());
    let tmp_path = path.with_file_name(tmp_name);
    fs::write(&tmp_path, content)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to atomically move {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn acquire_memory_lock(paths: &MoonPaths) -> Result<fs::File> {
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("failed to create {}", paths.logs_dir.display()))?;
    let lock_path = paths.logs_dir.join(MEMORY_LOCK_FILE);
    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(lock_file)
}

fn acquire_l1_normalisation_lock(paths: &MoonPaths) -> Result<fs::File> {
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("failed to create {}", paths.logs_dir.display()))?;
    let lock_path = paths.logs_dir.join(L1_NORM_LOCK_FILE);
    let lock_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(lock_file),
        Err(err) if err.kind() == ErrorKind::WouldBlock => {
            anyhow::bail!("l1 normalisation lock is already held")
        }
        Err(err) => Err(err).with_context(|| format!("failed to lock {}", lock_path.display())),
    }
}

fn append_distill_audit_event(paths: &MoonPaths, event: &DistillAuditEvent) -> Result<String> {
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("failed to create {}", paths.logs_dir.display()))?;
    let path = paths.logs_dir.join(DISTILL_AUDIT_FILE);
    let line = format!("{}\n", serde_json::to_string(event)?);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))?;
    Ok(path.display().to_string())
}

fn push_unique_limited(
    out: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    raw: &str,
    max_items: usize,
) {
    if out.len() >= max_items {
        return;
    }
    let Some(cleaned) = clean_candidate_text(raw) else {
        return;
    };
    if seen.insert(cleaned.to_ascii_lowercase()) {
        out.push(cleaned);
    }
}

fn extract_layer1_memory_lines(daily_memory: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut user = Vec::new();
    let mut assistant = Vec::new();
    let mut exec = Vec::new();
    let mut in_exec = false;

    for raw_line in daily_memory.lines() {
        let line = raw_line.trim();
        if line.starts_with("### Execution Summary") {
            in_exec = true;
            continue;
        }
        if line.starts_with("### ") && !line.starts_with("### Execution Summary") {
            in_exec = false;
        }
        if let Some(rest) = line.strip_prefix("**User:**") {
            user.push(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("**Assistant:**") {
            assistant.push(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("- [user]") {
            user.push(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = line.strip_prefix("- [assistant]") {
            assistant.push(rest.trim().to_string());
            continue;
        }
        if in_exec && line.starts_with("- ") {
            exec.push(line.trim_start_matches("- ").trim().to_string());
        }
    }

    (user, assistant, exec)
}

fn local_wisdom_sections(
    daily_memory: &str,
    current_memory: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (user_lines, assistant_lines, execution_lines) = extract_layer1_memory_lines(daily_memory);
    let mut lessons = Vec::new();
    let mut prefs = Vec::new();
    let mut durable = Vec::new();
    let mut lessons_seen = BTreeSet::new();
    let mut prefs_seen = BTreeSet::new();
    let mut durable_seen = BTreeSet::new();

    let pref_keywords = [
        "prefer", "like", "likes", "want", "wants", "please", "must", "should", "always", "never",
        "no ",
    ];
    for line in &user_lines {
        let lower = line.to_ascii_lowercase();
        if pref_keywords.iter().any(|kw| lower.contains(kw)) {
            push_unique_limited(
                &mut prefs,
                &mut prefs_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
        if lower.contains("decision")
            || lower.contains("rule")
            || lower.contains("keep")
            || lower.contains("use")
        {
            push_unique_limited(
                &mut durable,
                &mut durable_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }

    let mut user_counts = BTreeMap::<String, usize>::new();
    for line in &user_lines {
        let key = line.to_ascii_lowercase();
        *user_counts.entry(key).or_insert(0) += 1;
    }
    for line in &user_lines {
        let key = line.to_ascii_lowercase();
        if user_counts.get(&key).copied().unwrap_or(0) >= 2 {
            push_unique_limited(
                &mut prefs,
                &mut prefs_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }

    for line in &execution_lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("outcome")
            || lower.contains("lesson")
            || lower.contains("blocker")
            || lower.contains("retry")
        {
            push_unique_limited(
                &mut lessons,
                &mut lessons_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }

    for line in &assistant_lines {
        let lower = line.to_ascii_lowercase();
        if lower.contains("fixed")
            || lower.contains("resolved")
            || lower.contains("learned")
            || lower.contains("failed")
            || lower.contains("retry")
        {
            push_unique_limited(
                &mut lessons,
                &mut lessons_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
        if lower.contains("decision")
            || lower.contains("rule")
            || lower.contains("must")
            || lower.contains("keep")
        {
            push_unique_limited(
                &mut durable,
                &mut durable_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }

    if lessons.is_empty() && !execution_lines.is_empty() {
        for line in execution_lines.iter().take(3) {
            push_unique_limited(
                &mut lessons,
                &mut lessons_seen,
                line,
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }
    if lessons.is_empty() {
        push_unique_limited(
            &mut lessons,
            &mut lessons_seen,
            "Completed daily synthesis and retained actionable signals.",
            MAX_WISDOM_ITEMS_PER_SECTION,
        );
    }
    if prefs.is_empty() {
        push_unique_limited(
            &mut prefs,
            &mut prefs_seen,
            "No explicit repeated preference was detected today.",
            MAX_WISDOM_ITEMS_PER_SECTION,
        );
    }
    if durable.is_empty() {
        if !current_memory.trim().is_empty() {
            push_unique_limited(
                &mut durable,
                &mut durable_seen,
                "Preserved prior durable context from existing MEMORY.md.",
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        } else {
            push_unique_limited(
                &mut durable,
                &mut durable_seen,
                "No new durable decision was identified today.",
                MAX_WISDOM_ITEMS_PER_SECTION,
            );
        }
    }

    (lessons, prefs, durable)
}

fn render_wisdom_summary(lessons: &[String], prefs: &[String], durable: &[String]) -> String {
    let mut out = String::new();
    out.push_str("## Lessons Learned\n");
    for line in lessons.iter().take(MAX_WISDOM_ITEMS_PER_SECTION) {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');

    out.push_str("## User Preferences\n");
    for line in prefs.iter().take(MAX_WISDOM_ITEMS_PER_SECTION) {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');

    out.push_str("## Durable Decisions & Context\n");
    for line in durable.iter().take(MAX_WISDOM_ITEMS_PER_SECTION) {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn normalize_wisdom_summary(raw: &str, daily_memory: &str, current_memory: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Lessons,
        Prefs,
        Durable,
        Unknown,
    }

    let mut section = Section::Unknown;
    let mut lessons = Vec::new();
    let mut prefs = Vec::new();
    let mut durable = Vec::new();
    let mut lessons_seen = BTreeSet::new();
    let mut prefs_seen = BTreeSet::new();
    let mut durable_seen = BTreeSet::new();

    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("##") || line.starts_with('#') {
            let lower = line.to_ascii_lowercase();
            section = if lower.contains("lesson") {
                Section::Lessons
            } else if lower.contains("preference") || lower.contains("like") {
                Section::Prefs
            } else if lower.contains("durable")
                || lower.contains("decision")
                || lower.contains("context")
            {
                Section::Durable
            } else {
                Section::Unknown
            };
            continue;
        }

        let normalized = line
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim();
        if normalized.is_empty()
            || normalized.starts_with("**User:**")
            || normalized.starts_with("**Assistant:**")
        {
            continue;
        }

        let lower = normalized.to_ascii_lowercase();
        let target = if section != Section::Unknown {
            section
        } else if lower.contains("prefer")
            || lower.contains("like")
            || lower.contains("repeat")
            || lower.contains("wants")
        {
            Section::Prefs
        } else if lower.contains("decision")
            || lower.contains("rule")
            || lower.contains("context")
            || lower.contains("durable")
        {
            Section::Durable
        } else {
            Section::Lessons
        };

        match target {
            Section::Lessons => push_unique_limited(
                &mut lessons,
                &mut lessons_seen,
                normalized,
                MAX_WISDOM_ITEMS_PER_SECTION,
            ),
            Section::Prefs => push_unique_limited(
                &mut prefs,
                &mut prefs_seen,
                normalized,
                MAX_WISDOM_ITEMS_PER_SECTION,
            ),
            Section::Durable => push_unique_limited(
                &mut durable,
                &mut durable_seen,
                normalized,
                MAX_WISDOM_ITEMS_PER_SECTION,
            ),
            Section::Unknown => {}
        }
    }

    let (fallback_lessons, fallback_prefs, fallback_durable) =
        local_wisdom_sections(daily_memory, current_memory);
    if lessons.is_empty() {
        lessons = fallback_lessons;
    }
    if prefs.is_empty() {
        prefs = fallback_prefs;
    }
    if durable.is_empty() {
        durable = fallback_durable;
    }
    render_wisdom_summary(&lessons, &prefs, &durable)
}

fn validate_wisdom_summary(summary: &str) -> Result<()> {
    let lower = summary.to_ascii_lowercase();
    if !lower.contains("## lessons learned") {
        anyhow::bail!("wisdom summary missing `Lessons Learned` section");
    }
    if !lower.contains("## user preferences") {
        anyhow::bail!("wisdom summary missing `User Preferences` section");
    }
    if !lower.contains("## durable decisions & context") {
        anyhow::bail!("wisdom summary missing `Durable Decisions & Context` section");
    }
    if summary.lines().count() > MAX_WISDOM_LINES {
        anyhow::bail!("wisdom summary exceeds concise line budget");
    }
    if summary.contains("**User:**") || summary.contains("**Assistant:**") {
        anyhow::bail!("wisdom summary contains raw dialogue markers");
    }
    Ok(())
}

fn build_wisdom_prompt(day_key: &str, daily_memory: &str, current_memory: &str) -> String {
    format!(
        concat!(
            "You are maintaining MEMORY.md from daily conversation memory.\n",
            "Date: {day_key}\n",
            "Return markdown only with exactly these sections:\n",
            "## Lessons Learned\n",
            "## User Preferences\n",
            "## Durable Decisions & Context\n",
            "Rules:\n",
            "- Keep concise, high-signal bullets only.\n",
            "- Prefer repeated user preferences and durable decisions.\n",
            "- Do not include raw dialogue transcripts.\n",
            "- Merge with existing MEMORY context and avoid duplicates.\n\n",
            "Current MEMORY.md:\n{current_memory}\n\n",
            "Today's daily memory:\n{daily_memory}\n"
        ),
        day_key = day_key,
        current_memory = current_memory,
        daily_memory = daily_memory
    )
}

fn build_wisdom_chunk_prompt(
    day_key: &str,
    chunk_index: usize,
    chunk_total: usize,
    daily_chunk: &str,
    current_memory: &str,
) -> String {
    format!(
        concat!(
            "You are maintaining MEMORY.md from daily conversation memory.\n",
            "Date: {day_key}\n",
            "Chunk: {chunk_index}/{chunk_total}\n",
            "Return markdown only with exactly these sections:\n",
            "## Lessons Learned\n",
            "## User Preferences\n",
            "## Durable Decisions & Context\n",
            "Rules:\n",
            "- Keep concise, high-signal bullets only.\n",
            "- Prefer repeated user preferences and durable decisions.\n",
            "- Do not include raw dialogue transcripts.\n",
            "- Treat this as partial input; preserve only durable points.\n\n",
            "Current MEMORY.md (bounded):\n{current_memory}\n\n",
            "Daily memory chunk:\n{daily_chunk}\n"
        ),
        day_key = day_key,
        chunk_index = chunk_index,
        chunk_total = chunk_total,
        current_memory = current_memory,
        daily_chunk = daily_chunk
    )
}

fn truncate_text_to_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }
    if max_bytes <= 32 {
        return truncate_with_ellipsis(text, max_bytes);
    }

    let limit = max_bytes.saturating_sub(28);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf8();
        if used.saturating_add(width) > limit {
            break;
        }
        out.push(ch);
        used = used.saturating_add(width);
    }
    out.push_str("\n[truncated]");
    out
}

fn split_text_by_max_bytes(text: &str, max_chunk_bytes: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![String::new()];
    }
    if max_chunk_bytes == 0 {
        return vec![truncate_text_to_bytes(text, WISDOM_MIN_DAILY_CHUNK_BYTES)];
    }
    if text.len() <= max_chunk_bytes {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_bytes = 0usize;

    for line in text.lines() {
        let line_with_nl = format!("{line}\n");
        let line_bytes = line_with_nl.len();

        if line_bytes > max_chunk_bytes {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            chunks.push(truncate_text_to_bytes(&line_with_nl, max_chunk_bytes));
            continue;
        }

        if current_bytes.saturating_add(line_bytes) > max_chunk_bytes && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }

        current.push_str(&line_with_nl);
        current_bytes = current_bytes.saturating_add(line_bytes);
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(truncate_text_to_bytes(text, max_chunk_bytes));
    }
    chunks
}

fn detect_wisdom_context_tokens(remote: &RemoteModelConfig) -> u64 {
    if let Some(tokens) = parse_env_u64("MOON_WISDOM_CONTEXT_TOKENS") {
        return tokens;
    }
    if let Some(tokens) = detect_context_tokens_from_remote(remote) {
        return tokens;
    }
    infer_context_tokens_from_model(remote.provider, &remote.model)
}

fn resolve_wisdom_remote_config() -> Result<Option<RemoteModelConfig>> {
    let raw_provider = env_non_empty("MOON_WISDOM_PROVIDER").ok_or_else(|| {
        anyhow::anyhow!(
            "syns skipped: missing MOON_WISDOM_PROVIDER. Configure MOON_WISDOM_PROVIDER and MOON_WISDOM_MODEL for `moon distill -mode syns`."
        )
    })?;
    if raw_provider.eq_ignore_ascii_case("local") {
        return Ok(None);
    }

    let provider = parse_provider_alias(&raw_provider).ok_or_else(|| {
        anyhow::anyhow!(
            "syns skipped: invalid MOON_WISDOM_PROVIDER `{}`. Use one of: openai, openai-codex, anthropic, gemini, openai-compatible, local.",
            raw_provider
        )
    })?;

    let model = env_non_empty("MOON_WISDOM_MODEL").ok_or_else(|| {
        anyhow::anyhow!(
            "syns skipped: missing MOON_WISDOM_MODEL. Configure a primary synthesis model (for example gpt-4.1)."
        )
    })?;
    let (_, normalized_model) = parse_prefixed_model(&model);
    if normalized_model.trim().is_empty() {
        anyhow::bail!("syns skipped: MOON_WISDOM_MODEL is empty after normalization");
    }

    let base_url = match provider {
        RemoteProvider::OpenAiCodex => Some(resolve_openai_codex_base_url()),
        RemoteProvider::OpenAiCompatible => resolve_compatible_base_url(&normalized_model),
        _ => None,
    };
    let api_key = resolve_api_key(provider)?.ok_or_else(|| {
        anyhow::anyhow!(
            "syns skipped: missing API key for provider `{}`. Fix the primary model credentials.",
            provider.label()
        )
    })?;

    Ok(Some(RemoteModelConfig {
        provider,
        model: normalized_model,
        api_key,
        base_url,
    }))
}

fn call_remote_prompt(remote: &RemoteModelConfig, prompt: &str) -> Result<String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;

    match remote.provider {
        RemoteProvider::Gemini => {
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                remote.model, remote.api_key
            );
            let payload = serde_json::json!({
                "contents": [
                    {
                        "parts": [{"text": prompt}]
                    }
                ]
            });
            let response = client.post(&url).json(&payload).send()?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "gemini wisdom call failed with status {}",
                    response.status()
                );
            }
            let json: Value = response.json()?;
            let text = json
                .get("candidates")
                .and_then(Value::as_array)
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("content"))
                .and_then(|v| v.get("parts"))
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str)
                .context("gemini wisdom response missing text content")?;
            Ok(text.to_string())
        }
        RemoteProvider::OpenAi => {
            let payload = serde_json::json!({
                "model": remote.model,
                "input": prompt,
                "temperature": 0.2
            });
            let response = client
                .post("https://api.openai.com/v1/responses")
                .bearer_auth(&remote.api_key)
                .json(&payload)
                .send()?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "openai wisdom call failed with status {}",
                    response.status()
                );
            }
            let json: Value = response.json()?;
            extract_openai_text(&json).context("openai wisdom response missing text content")
        }
        RemoteProvider::OpenAiCodex => call_openai_codex_prompt(
            &client,
            remote.base_url.as_deref(),
            &remote.api_key,
            &remote.model,
            prompt,
            "wisdom",
        ),
        RemoteProvider::Anthropic => {
            let payload = serde_json::json!({
                "model": remote.model,
                "max_tokens": 1400,
                "temperature": 0.2,
                "messages": [{"role":"user", "content": prompt}]
            });
            let response = client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &remote.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&payload)
                .send()?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "anthropic wisdom call failed with status {}",
                    response.status()
                );
            }
            let json: Value = response.json()?;
            extract_anthropic_text(&json).context("anthropic wisdom response missing text content")
        }
        RemoteProvider::OpenAiCompatible => {
            let base = remote
                .base_url
                .as_deref()
                .unwrap_or("https://api.openai.com")
                .trim_end_matches('/');
            let url = format!("{base}/v1/chat/completions");
            let payload = serde_json::json!({
                "model": remote.model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": 0.2
            });
            let response = client
                .post(&url)
                .bearer_auth(&remote.api_key)
                .json(&payload)
                .send()?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "openai-compatible wisdom call failed with status {}",
                    response.status()
                );
            }
            let json: Value = response.json()?;
            extract_openai_compatible_text(&json)
                .context("openai-compatible wisdom response missing text content")
        }
    }
}

fn generate_wisdom_summary(
    day_key: &str,
    daily_memory: &str,
    current_memory: &str,
) -> Result<(String, String)> {
    if let Some(remote) = resolve_wisdom_remote_config()? {
        let context_tokens = detect_wisdom_context_tokens(&remote);
        let context_budget_bytes =
            token_limit_to_bytes_with_ratio(context_tokens, WISDOM_CONTEXT_SAFETY_RATIO);
        let bounded_current_budget = context_budget_bytes
            .saturating_div(3)
            .max(WISDOM_MIN_DAILY_CHUNK_BYTES);
        let bounded_current_memory = truncate_text_to_bytes(current_memory, bounded_current_budget);

        let daily_chunk_budget = context_budget_bytes
            .saturating_sub(bounded_current_memory.len())
            .saturating_sub(WISDOM_PROMPT_OVERHEAD_BYTES)
            .max(WISDOM_MIN_DAILY_CHUNK_BYTES);
        let daily_chunks = split_text_by_max_bytes(daily_memory, daily_chunk_budget);

        let mut partial_summaries = Vec::new();
        let mut first_remote_error: Option<anyhow::Error> = None;
        for (idx, chunk) in daily_chunks.iter().enumerate() {
            let mut chunk_body = chunk.clone();
            let mut prompt = build_wisdom_chunk_prompt(
                day_key,
                idx + 1,
                daily_chunks.len(),
                &chunk_body,
                &bounded_current_memory,
            );

            while prompt.len() > context_budget_bytes
                && chunk_body.len() > WISDOM_MIN_DAILY_CHUNK_BYTES
            {
                let next_budget = chunk_body.len().saturating_mul(8).saturating_div(10);
                chunk_body = truncate_text_to_bytes(&chunk_body, next_budget);
                prompt = build_wisdom_chunk_prompt(
                    day_key,
                    idx + 1,
                    daily_chunks.len(),
                    &chunk_body,
                    &bounded_current_memory,
                );
            }

            if prompt.len() > context_budget_bytes {
                continue;
            }

            match call_remote_prompt(&remote, &prompt) {
                Ok(raw) => {
                    let normalized = normalize_wisdom_summary(&raw, &chunk_body, current_memory);
                    partial_summaries.push(normalized);
                }
                Err(err) => {
                    if first_remote_error.is_none() {
                        first_remote_error = Some(err);
                    }
                }
            }
        }

        if !partial_summaries.is_empty() {
            let merged = if partial_summaries.len() == 1 {
                partial_summaries.remove(0)
            } else {
                normalize_wisdom_summary(
                    &partial_summaries.join("\n\n"),
                    daily_memory,
                    current_memory,
                )
            };
            return Ok((remote.provider.label().to_string(), merged));
        }

        // Single bounded attempt before failing synthesis for this run.
        let bounded_daily = truncate_text_to_bytes(
            daily_memory,
            context_budget_bytes
                .saturating_sub(bounded_current_memory.len())
                .saturating_sub(WISDOM_PROMPT_OVERHEAD_BYTES)
                .max(WISDOM_MIN_DAILY_CHUNK_BYTES),
        );
        let prompt = build_wisdom_prompt(day_key, &bounded_daily, &bounded_current_memory);
        if prompt.len() <= context_budget_bytes
            && let Ok(raw) = call_remote_prompt(&remote, &prompt)
        {
            let normalized = normalize_wisdom_summary(&raw, daily_memory, current_memory);
            return Ok((remote.provider.label().to_string(), normalized));
        }

        if let Some(err) = first_remote_error {
            return Err(err).context(
                "syns skipped: configured primary model failed. Fix MOON_WISDOM_PROVIDER / MOON_WISDOM_MODEL and provider credentials.",
            );
        }
        anyhow::bail!(
            "syns skipped: configured primary model produced no usable output. Fix MOON_WISDOM_PROVIDER / MOON_WISDOM_MODEL and retry."
        );
    }

    let (lessons, prefs, durable) = local_wisdom_sections(daily_memory, current_memory);
    Ok((
        "local".to_string(),
        render_wisdom_summary(&lessons, &prefs, &durable),
    ))
}

pub fn run_distillation(paths: &MoonPaths, input: &DistillInput) -> Result<DistillOutput> {
    fs::create_dir_all(&paths.memory_dir)
        .with_context(|| format!("failed to create {}", paths.memory_dir.display()))?;
    let _lock_file = acquire_l1_normalisation_lock(paths)?;

    let source_is_markdown = Path::new(&input.archive_path)
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));

    let (turns, execution_summary, message_count, filtered_noise_count) = if source_is_markdown {
        let projection_md = fs::read_to_string(&input.archive_path)
            .with_context(|| format!("failed to read {}", input.archive_path))?;
        extract_layer1_from_projection_markdown(&projection_md)
    } else {
        let projection = extract_projection_data(&input.archive_path)
            .with_context(|| format!("failed to parse archive {}", input.archive_path))?;
        let turns = projection
            .entries
            .iter()
            .filter_map(|entry| {
                if entry.role != "user" && entry.role != "assistant" {
                    return None;
                }
                normalize_turn_text(&entry.content).map(|text| (entry.role.clone(), text))
            })
            .collect::<Vec<_>>();
        let execution_summary = build_execution_summary_lines(&projection);
        (
            turns,
            execution_summary,
            projection.message_count,
            projection.filtered_noise_count,
        )
    };

    let summary = build_layer1_signal_summary(
        &input.session_id,
        &input.archive_path,
        &turns,
        execution_summary.as_deref(),
    );
    let session_block = render_layer1_session_block(
        input,
        message_count,
        filtered_noise_count,
        &turns,
        execution_summary.as_deref(),
    );

    let summary_path = daily_memory_path(paths, input.archive_epoch_secs);
    let date_label = Path::new(&summary_path)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("1970-01-01");
    let existing = fs::read_to_string(&summary_path).unwrap_or_default();
    let seeded = ensure_daily_memory_header(&existing, date_label);
    let (begin_marker, end_marker) = session_block_markers(&input.session_id);
    let full_text = upsert_marked_block(&seeded, &begin_marker, &end_marker, &session_block);

    fs::write(&summary_path, full_text)
        .with_context(|| format!("failed to write {}", summary_path))?;

    audit::append_event(
        paths,
        "distill",
        "ok",
        &format!(
            "l1_normalised session={} source={} target={}",
            input.session_id, input.archive_path, summary_path
        ),
    )?;

    Ok(DistillOutput {
        provider: "l1-normaliser".to_string(),
        summary,
        summary_path: summary_path.clone(),
        audit_log_path: paths.logs_dir.join("audit.log").display().to_string(),
        created_at_epoch_secs: now_epoch_secs()?,
    })
}

pub fn run_wisdom_distillation(
    paths: &MoonPaths,
    input: &WisdomDistillInput,
) -> Result<DistillOutput> {
    fs::create_dir_all(&paths.memory_dir)
        .with_context(|| format!("failed to create {}", paths.memory_dir.display()))?;
    fs::create_dir_all(&paths.logs_dir)
        .with_context(|| format!("failed to create {}", paths.logs_dir.display()))?;

    let epoch = input.day_epoch_secs.unwrap_or(now_epoch_secs()?);
    let default_today = today_daily_memory_path(paths, epoch);
    let memory_path = paths.memory_file.display().to_string();
    let explicit_sources = input
        .source_paths
        .iter()
        .any(|path| !path.trim().is_empty());

    let mut selected_sources = Vec::new();
    if explicit_sources {
        let mut seen = BTreeSet::new();
        for raw in &input.source_paths {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if seen.insert(trimmed.to_string()) {
                selected_sources.push(trimmed.to_string());
            }
        }
    } else {
        selected_sources.push(default_today.clone());
        selected_sources.push(memory_path.clone());
    }

    if selected_sources.is_empty() {
        anyhow::bail!("no synthesis source files provided");
    }

    let mut source_blocks = Vec::new();
    let mut participating_sources = Vec::new();
    for source_path in selected_sources {
        match fs::read_to_string(&source_path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    if explicit_sources {
                        anyhow::bail!("synthesis source file is empty: {}", source_path);
                    }
                    continue;
                }
                participating_sources.push(source_path.clone());
                source_blocks.push(format!("## Source: {}\n{}\n", source_path, trimmed));
            }
            Err(err) => {
                if !explicit_sources && source_path == memory_path {
                    // Default synthesis tolerates missing MEMORY.md and uses today's file only.
                    continue;
                }
                return Err(err)
                    .with_context(|| format!("failed to read synthesis source {}", source_path));
            }
        }
    }

    if participating_sources.is_empty() {
        anyhow::bail!("no non-empty synthesis sources available");
    }

    let synthesis_label = if explicit_sources {
        format!("files:{}", participating_sources.len())
    } else {
        "default:today+memory".to_string()
    };
    let synthesis_input = source_blocks.join("\n");
    let (provider, summary) = generate_wisdom_summary(&synthesis_label, &synthesis_input, "")
        .with_context(
            || "syns skipped: failed to run synthesis with the configured primary model",
        )?;
    validate_wisdom_summary(&summary)?;

    if input.dry_run {
        return Ok(DistillOutput {
            provider,
            summary,
            summary_path: paths.memory_file.display().to_string(),
            audit_log_path: paths
                .logs_dir
                .join(DISTILL_AUDIT_FILE)
                .display()
                .to_string(),
            created_at_epoch_secs: now_epoch_secs()?,
        });
    }

    let _lock_file = acquire_memory_lock(paths)?;
    let latest_memory = fs::read_to_string(&paths.memory_file).unwrap_or_default();
    let merged_memory = format!("# MEMORY\n\n{}\n", summary.trim_end());
    validate_wisdom_summary(&summary)?;

    let input_hash = sha256_hex(&synthesis_input);
    let output_hash = sha256_hex(&merged_memory);

    let previous_snapshot = latest_memory.clone();
    atomic_write_file(&paths.memory_file, &merged_memory)?;

    let event = DistillAuditEvent {
        at_epoch_secs: now_epoch_secs()?,
        mode: "syns".to_string(),
        trigger: input.trigger.clone(),
        source_path: participating_sources.join(";"),
        target_path: paths.memory_file.display().to_string(),
        input_hash,
        output_hash,
        provider: provider.clone(),
    };
    let audit_log_path = match append_distill_audit_event(paths, &event) {
        Ok(path) => path,
        Err(err) => {
            let _ = atomic_write_file(&paths.memory_file, &previous_snapshot);
            return Err(err);
        }
    };

    let _ = audit::append_event(
        paths,
        "distill",
        "ok",
        &format!(
            "mode=syns trigger={} sources={} target={} provider={}",
            input.trigger,
            participating_sources.join(";"),
            paths.memory_file.display(),
            provider
        ),
    );

    Ok(DistillOutput {
        provider,
        summary,
        summary_path: paths.memory_file.display().to_string(),
        audit_log_path,
        created_at_epoch_secs: now_epoch_secs()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkSummaryRollup, DistillInput, Distiller, LocalDistiller, MAX_SUMMARY_CHARS,
        RemoteProvider, WisdomDistillInput, clamp_summary, extract_anthropic_text,
        extract_openai_compatible_text, extract_openai_text, infer_provider_from_model,
        parse_prefixed_model, run_distillation, run_wisdom_distillation, sanitize_model_summary,
        stream_archive_chunks, summarize_provider_mix,
    };
    use crate::moon::paths::MoonPaths;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
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

    fn make_test_paths(root: &std::path::Path) -> MoonPaths {
        MoonPaths {
            moon_home: root.join("moon-home"),
            raw_dir: root.join("moon-home/raw"),
            mds_dir: root.join("moon-home/mds"),
            mlib_dir: root.join("moon-home/mlib"),
            cleanse_dir: root.join("moon-home/cleanse"),
            memory_dir: root.join("memory"),
            memory_file: root.join("MEMORY.md"),
            logs_dir: root.join("logs"),
            context_engine_dir: root.join("mce"),
            openclaw_sessions_dir: root.join("sessions"),
            qmd_bin: root.join("qmd"),
            qmd_db: root.join("qmd.db"),
            qmd_config_dir: root.join("qmd-config"),
            moon_home_is_explicit: true,
        }
    }

    #[test]
    fn local_distiller_avoids_raw_jsonl_payloads() {
        let input = DistillInput {
            session_id: "s".to_string(),
            archive_path: "/tmp/s.jsonl".to_string(),
            archive_text: format!(
                "{{\"type\":\"message\",\"message\":{{\"role\":\"toolResult\",\"content\":[{{\"type\":\"text\",\"text\":\"{{\\\"payload\\\":\\\"{}\\\"}}\"}}]}}}}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"Decision: set qmd mask to jsonl for archive indexing.\"}}]}}}}\n",
                "X".repeat(4096)
            ),
            archive_epoch_secs: None,
        };

        let summary = LocalDistiller
            .distill(&input)
            .expect("distill should succeed");
        assert!(summary.contains("Decision: set qmd mask to jsonl"));
        assert!(!summary.contains("\"payload\""));
        assert!(!summary.contains("\"type\":\"message\""));
    }

    #[test]
    fn clamp_summary_limits_large_output() {
        let giant = "A".repeat(MAX_SUMMARY_CHARS + 5000);
        let clamped = clamp_summary(&giant);
        assert!(clamped.chars().count() <= MAX_SUMMARY_CHARS + 32);
        assert!(clamped.contains("[summary truncated]"));
    }

    #[test]
    fn sanitize_model_summary_rejects_json_blob_output() {
        let raw = "{ \"type\": \"message\" }\n{ \"payload\": \"x\" }\n";
        assert!(sanitize_model_summary(raw).is_none());
    }

    #[test]
    fn sanitize_model_summary_normalizes_plain_lines_to_bullets() {
        let raw =
            "Decision: use jsonl mask\nRule: prefer concise bullets\nMilestone: qmd indexing fixed";
        let got = sanitize_model_summary(raw).expect("should produce summary");
        assert!(got.contains("- Decision: use jsonl mask"));
        assert!(got.contains("- Rule: prefer concise bullets"));
        assert!(got.contains("- Milestone: qmd indexing fixed"));
    }

    #[test]
    fn parse_prefixed_model_resolves_provider_hint() {
        let (provider, model) = parse_prefixed_model("openai:gpt-4.1-mini");
        assert_eq!(provider, Some(RemoteProvider::OpenAi));
        assert_eq!(model, "gpt-4.1-mini");

        let (provider, model) = parse_prefixed_model("openai-codex:gpt-5.4");
        assert_eq!(provider, Some(RemoteProvider::OpenAiCodex));
        assert_eq!(model, "gpt-5.4");

        let (provider, model) = parse_prefixed_model("claude:claude-3-5-haiku-latest");
        assert_eq!(provider, Some(RemoteProvider::Anthropic));
        assert_eq!(model, "claude-3-5-haiku-latest");

        let (provider, model) = parse_prefixed_model("deepseek:deepseek-chat");
        assert_eq!(provider, Some(RemoteProvider::OpenAiCompatible));
        assert_eq!(model, "deepseek-chat");
    }

    #[test]
    fn infer_provider_from_model_supports_openai_anthropic_and_gemini() {
        assert_eq!(
            infer_provider_from_model("gpt-4.1-mini"),
            Some(RemoteProvider::OpenAi)
        );
        assert_eq!(
            infer_provider_from_model("gpt-5.3-codex-spark"),
            Some(RemoteProvider::OpenAiCodex)
        );
        assert_eq!(
            infer_provider_from_model("claude-3-5-haiku-latest"),
            Some(RemoteProvider::Anthropic)
        );
        assert_eq!(
            infer_provider_from_model("gemini-2.5-flash-lite"),
            Some(RemoteProvider::Gemini)
        );
        assert_eq!(
            infer_provider_from_model("deepseek-chat"),
            Some(RemoteProvider::OpenAiCompatible)
        );
    }

    #[test]
    fn extract_openai_text_prefers_output_text_field() {
        let payload = json!({
            "output_text": "hello from openai"
        });
        assert_eq!(
            extract_openai_text(&payload).as_deref(),
            Some("hello from openai")
        );
    }

    #[test]
    fn extract_anthropic_text_reads_content_blocks() {
        let payload = json!({
            "content": [
                {"type": "text", "text": "line one"},
                {"type": "text", "text": "line two"}
            ]
        });
        assert_eq!(
            extract_anthropic_text(&payload).as_deref(),
            Some("line one\nline two")
        );
    }

    #[test]
    fn extract_openai_compatible_text_reads_chat_completions_shape() {
        let payload = json!({
            "choices": [
                {
                    "message": {
                        "content": "hello from compatible provider"
                    }
                }
            ]
        });
        assert_eq!(
            extract_openai_compatible_text(&payload).as_deref(),
            Some("hello from compatible provider")
        );
    }

    #[test]
    fn chunk_rollup_groups_keyword_sections() {
        let mut rollup = ChunkSummaryRollup::default();
        rollup.ingest_summary(
            "- Decision: enable chunk distill\n- Rule: keep archive gate at 2MB\n- Milestone: watcher can process 10MB archives\n- Open task: tune chunk size by workload",
        );

        let rendered = rollup.render("session-1", "/tmp/a.jsonl", 4, 524_288, 128, false);
        assert!(rendered.contains("### Decisions"));
        assert!(rendered.contains("### Rules"));
        assert!(rendered.contains("### Milestones"));
        assert!(rendered.contains("### Open Tasks"));
    }

    #[test]
    fn stream_archive_chunks_splits_input_by_target_size() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moon-chunk-test-{stamp}.jsonl"));
        fs::write(&path, "line-one\nline-two\nline-three\n").expect("write test file");

        let mut chunks = Vec::new();
        let path_str = path.to_string_lossy().to_string();
        let (count, truncated) = stream_archive_chunks(&path_str, 10, 16, |idx, text| {
            chunks.push((idx, text));
            Ok(())
        })
        .expect("chunking should succeed");

        let _ = fs::remove_file(&path);

        assert_eq!(count, 3);
        assert!(!truncated);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].1.contains("line-one"));
        assert!(chunks[1].1.contains("line-two"));
        assert!(chunks[2].1.contains("line-three"));
    }

    #[test]
    fn summarize_provider_mix_reports_mixed_counts() {
        let mut counts = BTreeMap::new();
        counts.insert("local".to_string(), 2usize);
        counts.insert("gemini".to_string(), 3usize);
        let label = summarize_provider_mix(&counts);
        assert!(label.starts_with("mixed("));
        assert!(label.contains("local:2"));
        assert!(label.contains("gemini:3"));
    }

    #[test]
    fn extract_projection_data_uses_min_max_timestamps() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moon-projection-time-test-{stamp}.jsonl"));
        let path_str = path.to_string_lossy().to_string();

        let line1 = json!({
            "message": {
                "role": "assistant",
                "createdAt": "2026-02-21T10:00:00Z",
                "content": [{"type":"text","text":"later event"}]
            }
        });
        let line2 = json!({
            "message": {
                "role": "user",
                "createdAt": "2026-02-21T09:00:00Z",
                "content": [{"type":"text","text":"earlier event"}]
            }
        });
        let line3 = "non-json system text";
        fs::write(&path, format!("{line1}\n{line2}\n{line3}\n")).expect("write test file");

        let data = super::extract_projection_data(&path_str).expect("extract projection data");
        let _ = fs::remove_file(&path);

        let early = chrono::DateTime::parse_from_rfc3339("2026-02-21T09:00:00Z")
            .expect("parse early")
            .timestamp() as u64;
        let late = chrono::DateTime::parse_from_rfc3339("2026-02-21T10:00:00Z")
            .expect("parse late")
            .timestamp() as u64;

        assert_eq!(data.time_start_epoch, Some(early));
        assert_eq!(data.time_end_epoch, Some(late));
    }

    #[test]
    fn extract_projection_data_accepts_numeric_and_top_level_timestamps() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moon-projection-ts-shapes-{stamp}.jsonl"));
        let path_str = path.to_string_lossy().to_string();

        let line1 = json!({
            "timestamp": "2026-02-18T05:23:52.625Z",
            "message": {
                "role": "assistant",
                "timestamp": 1771392232624u64,
                "content": [{"type":"text","text":"from numeric milliseconds"}]
            }
        });
        let line2 = json!({
            "timestamp": "2026-02-18T05:24:12.000Z",
            "message": {
                "role": "user",
                "content": [{"type":"text","text":"from top-level RFC3339"}]
            }
        });
        fs::write(&path, format!("{line1}\n{line2}\n")).expect("write test file");

        let data = super::extract_projection_data(&path_str).expect("extract projection data");
        let _ = fs::remove_file(&path);

        let early = 1_771_392_232u64;
        let late = chrono::DateTime::parse_from_rfc3339("2026-02-18T05:24:12Z")
            .expect("parse late")
            .timestamp() as u64;
        assert_eq!(data.time_start_epoch, Some(early));
        assert_eq!(data.time_end_epoch, Some(late));
    }

    #[test]
    fn extract_projection_data_keeps_tool_input_lexical_signals() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moon-projection-tool-signal-{stamp}.jsonl"));
        let path_str = path.to_string_lossy().to_string();

        let line = json!({
            "message": {
                "role": "assistant",
                "timestamp": 1771392232624u64,
                "content": [{
                    "type": "toolUse",
                    "name": "image_gen",
                    "input": {
                        "prompt": "Michelle pink luxury tweed suit with pearl buttons"
                    }
                }]
            }
        });
        fs::write(&path, format!("{line}\n")).expect("write test file");

        let data = super::extract_projection_data(&path_str).expect("extract projection data");
        let _ = fs::remove_file(&path);

        let merged = data
            .entries
            .iter()
            .map(|entry| entry.content.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(merged.contains("pink luxury tweed suit"));
    }

    #[test]
    fn extract_projection_data_keeps_tool_call_command_prompt_signal() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("moon-projection-toolcall-signal-{stamp}.jsonl"));
        let path_str = path.to_string_lossy().to_string();

        let line = json!({
            "message": {
                "role": "assistant",
                "timestamp": 1771392232624u64,
                "content": [{
                    "type": "toolCall",
                    "name": "exec",
                    "arguments": {
                        "command": "uv run generate_image.py --prompt \"Michelle in a pink luxury tweed suit, studio fashion lighting\" --resolution 2K"
                    }
                }]
            }
        });
        fs::write(&path, format!("{line}\n")).expect("write test file");

        let data = super::extract_projection_data(&path_str).expect("extract projection data");
        let _ = fs::remove_file(&path);

        let merged = data
            .entries
            .iter()
            .map(|entry| entry.content.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(merged.contains("pink luxury tweed suit"));
    }

    #[test]
    fn extract_projection_data_filters_noise_markers_and_poll_chatter() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moon-projection-noise-filter-{stamp}.jsonl"));
        let path_str = path.to_string_lossy().to_string();

        let no_reply = json!({
            "message": {
                "role": "assistant",
                "content": [{"type":"text","text":"NO_REPLY"}]
            }
        });
        let poll_noise = json!({
            "message": {
                "role": "toolResult",
                "content": [{
                    "type":"text",
                    "text":"Command still running (session quiet-orbit, pid 86331). Use process (list/poll/log/write/kill/clear/remove) for follow-up."
                }]
            }
        });
        let meaningful = json!({
            "message": {
                "role": "user",
                "content": [{"type":"text","text":"Decision: keep trigger ratio at 1.0."}]
            }
        });
        fs::write(&path, format!("{no_reply}\n{poll_noise}\n{meaningful}\n"))
            .expect("write test file");

        let data = super::extract_projection_data(&path_str).expect("extract projection data");
        let _ = fs::remove_file(&path);

        assert_eq!(data.filtered_noise_count, 2);
        assert_eq!(data.entries.len(), 1);
        assert_eq!(data.entries[0].role, "user");
        assert!(
            data.entries[0]
                .content
                .contains("Decision: keep trigger ratio")
        );
    }

    #[test]
    fn test_extract_keywords() {
        let text = "We need to fix the WebGL rendering bug on Safari. Also investigate the auth-token expiration issue.";
        let entry = super::ProjectionEntry {
            timestamp_epoch: None,
            role: "user".to_string(),
            content: text.to_string(),
            tool_name: None,
            tool_target: None,
            priority: None,
            coupled_result: None,
        };
        let keywords = super::extract_keywords(&[entry]);
        assert!(
            keywords.contains(&"webgl".to_string())
                || keywords.contains(&"safari".to_string())
                || keywords.contains(&"auth-token".to_string())
        );
    }

    #[test]
    fn semantic_dedup_keeps_latest_state_line() {
        let raw =
            "## Decisions\n- Trigger ratio: 0.85\n- Trigger ratio: 1.0\n- Keep archive snapshots\n";
        let deduped = super::apply_semantic_dedup(raw);
        assert!(!deduped.contains("Trigger ratio: 0.85"));
        assert!(deduped.contains("Trigger ratio: 1.0"));
        assert!(deduped.contains("Keep archive snapshots"));
    }

    #[test]
    fn upsert_entity_anchor_block_replaces_session_line() {
        let existing = "\
<!-- MOON_ENTITY_ANCHORS_BEGIN -->
## Entity Anchors
- session_id=s1 archive_path=/tmp/a topics=#moon #qmd
<!-- MOON_ENTITY_ANCHORS_END -->

### s1
## Distilled Session Summary
";
        let updated = super::upsert_entity_anchors_block(
            existing,
            "s1",
            "/tmp/a",
            &["#moon".to_string(), "#memory".to_string()],
        );
        assert!(updated.contains("topics=#moon #memory"));
        assert!(!updated.contains("topics=#moon #qmd"));
    }

    #[test]
    fn discover_topic_tags_filters_timestamp_noise() {
        let summary = "Decision: Keep moon trigger ratio at 1.0. archive_jsonl_path /tmp/x. 2026-02-21T10:00:00Z. QMD indexing stable.";
        let tags = super::discover_topic_tags(summary);
        assert!(tags.iter().any(|t| t == "#moon"));
        assert!(tags.iter().any(|t| t == "#qmd"));
        assert!(!tags.iter().any(|t| t.contains("2026")));
        assert!(!tags.iter().any(|t| t.contains("archive_jsonl_path")));
    }

    #[test]
    fn run_distillation_writes_conversation_first_daily_memory() {
        let tmp = tempdir().expect("tempdir");
        let paths = make_test_paths(tmp.path());
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.logs_dir).expect("mkdir logs");

        let archive = tmp.path().join("session.jsonl");
        let user = json!({
            "message": {
                "role": "user",
                "timestamp": 1_700_000_000u64,
                "content": [{"type":"text","text":"Please keep responses concise and actionable."}]
            }
        });
        let assistant_tool = json!({
            "message": {
                "role": "assistant",
                "timestamp": 1_700_000_001u64,
                "content": [
                    {"type":"text","text":"I will patch the parser and run tests."},
                    {"type":"toolUse","name":"exec","input":{"command":"cargo test"}}
                ]
            }
        });
        let tool_result = json!({
            "message": {
                "role": "toolResult",
                "timestamp": 1_700_000_002u64,
                "content": [{"type":"text","text":"test result: ok. 42 passed"}]
            }
        });
        let assistant = json!({
            "message": {
                "role": "assistant",
                "timestamp": 1_700_000_003u64,
                "content": [{"type":"text","text":"Done. Tests are passing."}]
            }
        });
        fs::write(
            &archive,
            format!("{user}\n{assistant_tool}\n{tool_result}\n{assistant}\n"),
        )
        .expect("write archive");

        let out = run_distillation(
            &paths,
            &DistillInput {
                session_id: "s1".to_string(),
                archive_path: archive.display().to_string(),
                archive_text: String::new(),
                archive_epoch_secs: Some(1_700_000_000),
            },
        )
        .expect("layer1 distill should succeed");

        let daily = fs::read_to_string(&out.summary_path).expect("read daily memory");
        assert!(daily.contains("moon_memory_format: conversation_v1"));
        assert!(daily.contains("## Session s1"));
        assert!(daily.contains("**User:** Please keep responses concise and actionable."));
        assert!(daily.contains("**Assistant:** I will patch the parser and run tests."));
        assert!(daily.contains("### Execution Summary"));
        assert!(!daily.contains("[tool-input]"));
    }

    #[test]
    fn run_distillation_accepts_projection_markdown_source() {
        let tmp = tempdir().expect("tempdir");
        let paths = make_test_paths(tmp.path());
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.logs_dir).expect("mkdir logs");

        let projection = tmp.path().join("session-projection.md");
        fs::write(
            &projection,
            r#"---
moon_archive_projection: 2
message_count: 7
filtered_noise_count: 2
---

## Conversations

### User Queries
- [10:00:01Z] Please keep answers concise.
- [10:00:20Z] Please keep answers concise.

### Assistant Responses
- [10:00:15Z] I will update the command flow and rerun tests.
- [10:00:50Z] Done, everything is green.

## Tool Activity
### exec
- [10:00:30Z] `cargo test` -> ok
"#,
        )
        .expect("write projection");

        let out = run_distillation(
            &paths,
            &DistillInput {
                session_id: "md1".to_string(),
                archive_path: projection.display().to_string(),
                archive_text: String::new(),
                archive_epoch_secs: Some(1_700_000_100),
            },
        )
        .expect("layer1 distill from projection should succeed");

        let daily = fs::read_to_string(out.summary_path).expect("read daily memory");
        assert!(daily.contains("## Session md1"));
        assert!(daily.contains("Please keep answers concise."));
        assert!(daily.contains("I will update the command flow and rerun tests."));
        assert!(daily.contains("### Execution Summary"));
    }

    #[test]
    fn split_text_by_max_bytes_produces_bounded_chunks() {
        let text = (0..2000)
            .map(|idx| format!("line {idx} keep this memory signal"))
            .collect::<Vec<_>>()
            .join("\n");
        let max_bytes = 1024usize;
        let chunks = super::split_text_by_max_bytes(&text, max_bytes);
        assert!(chunks.len() > 1);
        for chunk in chunks {
            assert!(chunk.len() <= max_bytes + 32);
        }
    }

    #[test]
    fn run_wisdom_distillation_updates_memory_file_and_audit_log() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("lock test env");
        let _provider = ScopedEnvVar::set("MOON_WISDOM_PROVIDER", "local");

        let tmp = tempdir().expect("tempdir");
        let paths = make_test_paths(tmp.path());
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.logs_dir).expect("mkdir logs");
        fs::write(&paths.memory_file, "# MEMORY\n").expect("write memory");

        let epoch = 1_700_000_000u64;
        let daily_path = super::daily_memory_path(&paths, Some(epoch));
        fs::write(
            &daily_path,
            r#"# Daily Memory 2023-11-14
<!-- moon_memory_format: conversation_v1 -->

## Session s1
### Conversation
**User:** I prefer concise answers.
**Assistant:** I updated the parser and tests.
**User:** I prefer concise answers.

### Execution Summary
- Goal: fix parser test failure
- Key actions: used `exec` on cargo test
- Outcome: tests pass
"#,
        )
        .expect("write daily");

        let out = run_wisdom_distillation(
            &paths,
            &WisdomDistillInput {
                trigger: "test".to_string(),
                day_epoch_secs: Some(epoch),
                source_paths: Vec::new(),
                dry_run: false,
            },
        )
        .expect("wisdom distill should succeed");

        let memory = fs::read_to_string(&paths.memory_file).expect("read memory");
        assert!(memory.starts_with("# MEMORY"));
        assert!(memory.contains("## Lessons Learned"));
        assert!(memory.contains("## User Preferences"));
        assert!(memory.contains("## Durable Decisions & Context"));
        assert!(!memory.contains("MOON_WISDOM_BEGIN"));
        assert_eq!(out.summary_path, paths.memory_file.display().to_string());

        let audit_path = PathBuf::from(&out.audit_log_path);
        let audit = fs::read_to_string(audit_path).expect("read distill audit log");
        assert!(audit.contains("\"mode\":\"syns\""));
        assert!(audit.contains("\"trigger\":\"test\""));
    }

    #[test]
    fn run_wisdom_distillation_explicit_sources_do_not_implicitly_include_memory() {
        let _env_lock = TEST_ENV_LOCK.lock().expect("lock test env");
        let _provider = ScopedEnvVar::set("MOON_WISDOM_PROVIDER", "local");

        let tmp = tempdir().expect("tempdir");
        let paths = make_test_paths(tmp.path());
        fs::create_dir_all(&paths.memory_dir).expect("mkdir memory");
        fs::create_dir_all(&paths.logs_dir).expect("mkdir logs");
        fs::write(
            &paths.memory_file,
            "# MEMORY\n\n## Lessons Learned\n- Legacy sentinel should be removed.\n",
        )
        .expect("write memory");

        let source = tmp.path().join("custom-source.md");
        fs::write(
            &source,
            r#"## Session x
### Conversation
**User:** Keep responses concise and practical.
**Assistant:** Updated implementation and test flow.

### Execution Summary
- Goal: verify targeted synthesis
- Key actions: selected explicit file sources only
- Outcome: done
"#,
        )
        .expect("write source");

        run_wisdom_distillation(
            &paths,
            &WisdomDistillInput {
                trigger: "explicit-sources".to_string(),
                day_epoch_secs: Some(1_700_000_000),
                source_paths: vec![source.display().to_string()],
                dry_run: false,
            },
        )
        .expect("wisdom distill should succeed");

        let memory = fs::read_to_string(&paths.memory_file).expect("read memory");
        assert!(memory.starts_with("# MEMORY"));
        assert!(!memory.contains("Legacy sentinel should be removed"));
        assert!(memory.contains("## Lessons Learned"));
        assert!(memory.contains("## User Preferences"));
        assert!(memory.contains("## Durable Decisions & Context"));
    }
}
