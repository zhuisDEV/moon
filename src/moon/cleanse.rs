use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::env;

use crate::moon::openai_codex_auth;
use crate::moon::util::{now_epoch_secs, truncate_with_ellipsis};

const DEFAULT_CLEANSE_MODEL: &str = "gemini-3.1-flash-lite-preview";
const REQUEST_TIMEOUT_SECS: u64 = 45;
const MAX_SUMMARY_CHARS: usize = 16_000;
const MAX_MODEL_LINES: usize = 120;
const MIN_BULLET_LINES: usize = 3;
const DEFAULT_OPENAI_CODEX_MODEL: &str = "gpt-5.4";

#[derive(Debug, Clone)]
pub struct CleanseInput {
    pub session_id: String,
    pub source_path: String,
    pub source_excerpt: String,
}

#[derive(Debug, Clone)]
pub struct CleanseOutput {
    pub provider: String,
    pub model: String,
    pub summary: String,
    pub created_at_epoch_secs: u64,
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
            Self::OpenAi => "openai",
            Self::OpenAiCodex => "openai-codex",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenAiCompatible => "openai-compatible",
        }
    }
}

#[derive(Debug, Clone)]
struct CleanseModelConfig {
    provider: RemoteProvider,
    model: String,
    api_key: String,
    base_url: Option<String>,
}

pub fn run_cleanse(input: &CleanseInput) -> Result<CleanseOutput> {
    let config = resolve_cleanse_config()?;
    let prompt = build_cleanse_prompt(input);
    let raw_summary = match config.provider {
        RemoteProvider::OpenAi => call_openai(&config, &prompt)?,
        RemoteProvider::OpenAiCodex => call_openai_codex(&config, &prompt)?,
        RemoteProvider::Anthropic => call_anthropic(&config, &prompt)?,
        RemoteProvider::Gemini => call_gemini(&config, &prompt)?,
        RemoteProvider::OpenAiCompatible => call_openai_compatible(&config, &prompt)?,
    };
    let summary = sanitize_summary(&raw_summary).ok_or_else(|| {
        anyhow::anyhow!(
            "cleanse model produced no usable summary; refine MOON_CLEANSE_MODEL or retry"
        )
    })?;

    Ok(CleanseOutput {
        provider: config.provider.label().to_string(),
        model: config.model,
        summary: clamp_summary(&summary),
        created_at_epoch_secs: now_epoch_secs()?,
    })
}

pub fn resolved_cleanse_model_label() -> String {
    env_non_empty("MOON_CLEANSE_MODEL").unwrap_or_else(|| DEFAULT_CLEANSE_MODEL.to_string())
}

pub fn render_summary_document(
    session_id: &str,
    source_path: &str,
    provider: &str,
    model: &str,
    created_at_epoch_secs: u64,
    summary: &str,
) -> String {
    format!(
        "---\nmoon_cleanse: 1\nsession_id: {}\nsource_path: {}\nprovider: {}\nmodel: {}\ncreated_at_epoch_secs: {}\n---\n\n{}\n",
        serde_json::to_string(session_id).unwrap_or_else(|_| "\"session\"".to_string()),
        serde_json::to_string(source_path).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(provider).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(model).unwrap_or_else(|_| "\"\"".to_string()),
        created_at_epoch_secs,
        summary.trim_end()
    )
}

fn resolve_cleanse_config() -> Result<CleanseModelConfig> {
    let configured_provider = env_non_empty("MOON_CLEANSE_PROVIDER")
        .as_deref()
        .and_then(parse_provider_alias);
    if env_non_empty("MOON_CLEANSE_PROVIDER")
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("local"))
    {
        anyhow::bail!(
            "MOON_CLEANSE_PROVIDER=local is unsupported; cleanse requires a remote model"
        );
    }

    let configured_model =
        env_non_empty("MOON_CLEANSE_MODEL").unwrap_or_else(|| DEFAULT_CLEANSE_MODEL.to_string());
    let mut chosen_provider = configured_provider;
    let (prefixed_provider, mut model) = parse_prefixed_model(&configured_model);
    if chosen_provider.is_none() {
        chosen_provider = prefixed_provider.or_else(|| infer_provider_from_model(&model));
    }

    let mut provider = chosen_provider.ok_or_else(|| {
        anyhow::anyhow!(
            "failed to resolve cleanse provider; set MOON_CLEANSE_PROVIDER or prefix MOON_CLEANSE_MODEL"
        )
    })?;
    if model.trim().is_empty() {
        model = DEFAULT_CLEANSE_MODEL.to_string();
    }

    let mut api_key = resolve_api_key(provider)?;
    if api_key.is_none()
        && configured_provider.is_none()
        && prefixed_provider.is_none()
        && let Some(fallback_provider) = first_available_provider()
        && fallback_provider != provider
    {
        provider = fallback_provider;
        model = default_model_for_provider(provider).to_string();
        api_key = resolve_api_key(provider)?;
    }
    let api_key = api_key.ok_or_else(|| {
        anyhow::anyhow!(
            "missing provider credentials for cleanse; configure the relevant API key for {}",
            provider.label()
        )
    })?;
    let base_url = match provider {
        RemoteProvider::OpenAiCodex => Some(resolve_openai_codex_base_url()),
        RemoteProvider::OpenAiCompatible => Some(
            env_non_empty("AI_BASE_URL").unwrap_or_else(|| "https://api.openai.com".to_string()),
        ),
        _ => None,
    };

    Ok(CleanseModelConfig {
        provider,
        model,
        api_key,
        base_url,
    })
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
        RemoteProvider::OpenAiCodex => DEFAULT_OPENAI_CODEX_MODEL,
        RemoteProvider::Anthropic => "claude-3-5-haiku-latest",
        RemoteProvider::Gemini => DEFAULT_CLEANSE_MODEL,
        RemoteProvider::OpenAiCompatible => "deepseek-chat",
    }
}

fn build_cleanse_prompt(input: &CleanseInput) -> String {
    format!(
        "You are MOON cleanse. Compress the active context into a compact recovery summary for the moon-context-engine.\n\
Return markdown only.\n\
\n\
Requirements:\n\
- preserve the current goal, active subproblems, decisions, constraints, blockers, pending tasks, and important tool outcomes\n\
- remove repetition, pleasantries, decorative phrasing, low-signal chatter, and verbose logs\n\
- do not emit raw JSON, YAML, XML, code fences, or long verbatim transcripts\n\
- produce a concise summary suitable for recovering from context pressure near 60k tokens toward a safer working footprint around 40k tokens\n\
\n\
Format:\n\
# Cleanse Summary\n\
## Current Goal\n\
- ...\n\
## Active Context\n\
- ...\n\
## Decisions\n\
- ...\n\
## Open Tasks\n\
- ...\n\
## Risks / Blockers\n\
- ...\n\
## Relevant Evidence\n\
- ...\n\
\n\
Session id: {}\n\
Source path: {}\n\
\n\
Context excerpt:\n{}\n",
        input.session_id, input.source_path, input.source_excerpt
    )
}

fn call_gemini(config: &CleanseModelConfig, prompt: &str) -> Result<String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        config.model, config.api_key
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
        anyhow::bail!(
            "gemini cleanse call failed with status {}",
            response.status()
        );
    }

    let json: Value = response.json()?;
    json.get("candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("content"))
        .and_then(|item| item.get("parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
        .context("gemini cleanse response missing text content")
}

fn call_openai(config: &CleanseModelConfig, prompt: &str) -> Result<String> {
    let payload = serde_json::json!({
        "model": config.model,
        "input": prompt,
        "temperature": 0.2,
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;
    let response = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()?;
    if !response.status().is_success() {
        anyhow::bail!(
            "openai cleanse call failed with status {}",
            response.status()
        );
    }

    let json: Value = response.json()?;
    extract_openai_text(&json).context("openai cleanse response missing text content")
}

fn call_openai_codex(config: &CleanseModelConfig, prompt: &str) -> Result<String> {
    let base = config
        .base_url
        .as_deref()
        .unwrap_or("https://chatgpt.com/backend-api")
        .trim_end_matches('/');
    let url = if base.ends_with("/codex/responses") {
        base.to_string()
    } else {
        format!("{base}/codex/responses")
    };
    let payload = openai_codex_payload(&config.model, prompt);

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .header("accept", "text/event-stream")
        .json(&payload)
        .send()?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        anyhow::bail!(
            "openai-codex cleanse call failed with status {}: {}",
            status,
            truncate_with_ellipsis(body.trim(), 240)
        );
    }

    let body = response.text()?;
    extract_openai_codex_text(&body).context("openai-codex cleanse response missing text content")
}

fn openai_codex_payload(model: &str, prompt: &str) -> Value {
    serde_json::json!({
        "model": model,
        "instructions": "You are MOON. Execute the requested task exactly and return plain text only.",
        "input": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": prompt
                    }
                ]
            }
        ],
        "store": false,
        "stream": true,
    })
}

fn call_openai_compatible(config: &CleanseModelConfig, prompt: &str) -> Result<String> {
    let base = config
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com")
        .trim_end_matches('/');
    let url = format!("{base}/v1/chat/completions");
    let payload = serde_json::json!({
        "model": config.model,
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
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()?;
    if !response.status().is_success() {
        anyhow::bail!(
            "openai-compatible cleanse call failed with status {}",
            response.status()
        );
    }

    let json: Value = response.json()?;
    extract_openai_compatible_text(&json)
        .context("openai-compatible cleanse response missing text content")
}

fn call_anthropic(config: &CleanseModelConfig, prompt: &str) -> Result<String> {
    let payload = serde_json::json!({
        "model": config.model,
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
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&payload)
        .send()?;
    if !response.status().is_success() {
        anyhow::bail!(
            "anthropic cleanse call failed with status {}",
            response.status()
        );
    }

    let json: Value = response.json()?;
    extract_anthropic_text(&json).context("anthropic cleanse response missing text content")
}

fn sanitize_summary(summary: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut bullet_count = 0usize;

    for raw_line in summary.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if looks_like_structured_fragment(trimmed)
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

    if bullet_count < MIN_BULLET_LINES {
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

fn clean_candidate_text(raw: &str) -> Option<String> {
    let collapsed = raw
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect::<String>();
    let normalized = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn looks_like_structured_fragment(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with("```")
        || trimmed == "{"
        || trimmed == "}"
        || trimmed == "["
        || trimmed == "]"
        || trimmed.starts_with("{\"")
        || trimmed.starts_with("[{")
        || trimmed.starts_with("</")
        || trimmed.starts_with("<xml")
        || trimmed.starts_with("---")
}

fn env_non_empty(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
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

fn resolve_openai_codex_base_url() -> String {
    env_non_empty("OPENAI_CODEX_BASE_URL")
        .or_else(|| env_non_empty("OPENAI_BASE_URL"))
        .unwrap_or_else(|| "https://chatgpt.com/backend-api".to_string())
}

fn extract_openai_text(root: &Value) -> Option<String> {
    root.get("output")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
        .or_else(|| {
            root.get("output_text")
                .and_then(Value::as_str)
                .map(|text| text.to_string())
        })
}

fn extract_openai_compatible_text(root: &Value) -> Option<String> {
    root.get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("message"))
        .and_then(|item| item.get("content"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

fn extract_anthropic_text(root: &Value) -> Option<String> {
    root.get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

fn extract_openai_codex_text(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let json: Value = serde_json::from_str(trimmed).ok()?;
        return extract_openai_text(&json);
    }

    let mut latest_done = None;
    let mut deltas = String::new();
    for line in trimmed.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let event: Value = serde_json::from_str(data).ok()?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    deltas.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    latest_done = Some(text.to_string());
                }
            }
            _ => {}
        }
    }

    latest_done.or_else(|| (!deltas.is_empty()).then_some(deltas))
}

#[cfg(test)]
mod tests {
    use super::{extract_openai_codex_text, openai_codex_payload};

    #[test]
    fn openai_codex_payload_uses_instructions_and_structured_input() {
        let payload = openai_codex_payload("gpt-5.4", "Summarize this session.");

        assert_eq!(
            payload.get("model").and_then(|v| v.as_str()),
            Some("gpt-5.4")
        );
        assert!(
            payload
                .get("instructions")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty())
        );
        assert_eq!(payload.get("store").and_then(|v| v.as_bool()), Some(false));

        let input = payload
            .get("input")
            .and_then(|v| v.as_array())
            .expect("input list");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].get("role").and_then(|v| v.as_str()), Some("user"));

        let content = input[0]
            .get("content")
            .and_then(|v| v.as_array())
            .expect("content list");
        assert_eq!(content.len(), 1);
        assert_eq!(
            content[0].get("type").and_then(|v| v.as_str()),
            Some("input_text")
        );
        assert_eq!(
            content[0].get("text").and_then(|v| v.as_str()),
            Some("Summarize this session.")
        );
        assert_eq!(payload.get("stream").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn extract_openai_codex_text_reads_sse_output_text_done() {
        let body = r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"hel"}

event: response.output_text.done
data: {"type":"response.output_text.done","text":"hello"}
"#;

        assert_eq!(extract_openai_codex_text(body).as_deref(), Some("hello"));
    }
}
