use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rand::{RngCore, rngs::OsRng};
use reqwest::Url;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const DEFAULT_OPENAI_AUTH_BASE_URL: &str = "https://auth.openai.com";
const DEFAULT_OPENAI_AUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const DEFAULT_OPENAI_CALLBACK_BIND_ADDR: &str = "127.0.0.1:1455";
const OAUTH_LOGIN_TIMEOUT_SECS: u64 = 300;
const TOKEN_REFRESH_SKEW_SECS: u64 = 60;
const TOKEN_REQUEST_TIMEOUT_SECS: u64 = 30;
const MOON_AUTH_FILE_NAME: &str = "openai-codex.json";
const MOON_AUTH_LOCK_NAME: &str = "openai-codex.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginCallbackMode {
    BrowserCallback,
    ManualCode,
}

#[derive(Debug, Clone)]
pub struct LoginResult {
    pub auth_store_path: PathBuf,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub expires_at_epoch_ms: u64,
    pub browser_opened: bool,
    pub callback_mode: LoginCallbackMode,
}

#[derive(Debug, Clone)]
enum CredentialSource {
    MoonStore,
    CodexCliStore,
}

#[derive(Debug, Clone)]
struct OpenAiCodexCredential {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
    email: Option<String>,
    expires_at_epoch_ms: u64,
    source: CredentialSource,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
struct CodexAuthRecord {
    auth_mode: Option<String>,
    last_refresh: Option<String>,
    tokens: CodexAuthTokens,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
struct CodexAuthTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CodexJwtPayload {
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile")]
    profile: Option<CodexJwtProfile>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<CodexJwtAuth>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CodexJwtProfile {
    email: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CodexJwtAuth {
    chatgpt_account_id: Option<String>,
}

fn env_non_empty(var: &str) -> Option<String> {
    match env::var(var) {
        Ok(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn oauth_base_url() -> String {
    env_non_empty("MOON_OPENAI_OAUTH_BASE_URL")
        .unwrap_or_else(|| DEFAULT_OPENAI_AUTH_BASE_URL.to_string())
}

fn oauth_client_id() -> String {
    env_non_empty("MOON_OPENAI_OAUTH_CLIENT_ID")
        .unwrap_or_else(|| DEFAULT_OPENAI_AUTH_CLIENT_ID.to_string())
}

fn oauth_redirect_uri() -> String {
    env_non_empty("MOON_OPENAI_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|| DEFAULT_OPENAI_REDIRECT_URI.to_string())
}

fn oauth_callback_bind_addr() -> String {
    env_non_empty("MOON_OPENAI_OAUTH_CALLBACK_BIND_ADDR")
        .unwrap_or_else(|| DEFAULT_OPENAI_CALLBACK_BIND_ADDR.to_string())
}

fn oauth_authorize_url() -> Result<Url> {
    let mut url = Url::parse(&format!(
        "{}/oauth/authorize",
        oauth_base_url().trim_end_matches('/')
    ))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oauth_client_id())
        .append_pair("redirect_uri", &oauth_redirect_uri())
        .append_pair("scope", OAUTH_SCOPE)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true");
    Ok(url)
}

fn oauth_token_url() -> Result<String> {
    Ok(format!(
        "{}/oauth/token",
        oauth_base_url().trim_end_matches('/')
    ))
}

fn decode_jwt_payload(token: &str) -> Option<CodexJwtPayload> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _sig = parts.next()?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice::<CodexJwtPayload>(&decoded).ok()
}

fn decode_access_token_expiry_ms(token: &str) -> Option<u64> {
    decode_jwt_payload(token)?
        .exp
        .map(|value| value.saturating_mul(1_000))
}

fn decode_access_token_email(token: &str) -> Option<String> {
    decode_jwt_payload(token)?
        .profile?
        .email
        .and_then(|value| trim_non_empty(&value))
}

fn decode_access_token_account_id(token: &str) -> Option<String> {
    decode_jwt_payload(token)?
        .auth?
        .chatgpt_account_id
        .and_then(|value| trim_non_empty(&value))
}

fn trim_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_last_refresh_epoch_ms(last_refresh: Option<&str>) -> Option<u64> {
    let raw = last_refresh?;
    let parsed = DateTime::parse_from_rfc3339(raw).ok()?;
    u64::try_from(parsed.timestamp_millis()).ok()
}

fn fallback_expiry_ms(path: &Path, last_refresh: Option<&str>) -> u64 {
    if let Some(last_refresh_ms) = parse_last_refresh_epoch_ms(last_refresh) {
        return last_refresh_ms.saturating_add(60 * 60 * 1_000);
    }
    if let Ok(metadata) = fs::metadata(path)
        && let Ok(modified) = metadata.modified()
        && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        return duration
            .as_millis()
            .min(u128::from(u64::MAX))
            .try_into()
            .unwrap_or(u64::MAX)
            .saturating_add(60 * 60 * 1_000);
    }
    current_epoch_ms().saturating_add(60 * 60 * 1_000)
}

fn current_epoch_ms() -> u64 {
    let now = Utc::now().timestamp_millis();
    if now.is_negative() { 0 } else { now as u64 }
}

fn credential_is_expired(credential: &OpenAiCodexCredential) -> bool {
    credential.expires_at_epoch_ms
        <= current_epoch_ms().saturating_add(TOKEN_REFRESH_SKEW_SECS * 1_000)
}

fn moon_auth_dir() -> Result<PathBuf> {
    let paths = crate::moon::paths::resolve_paths()?;
    Ok(paths.moon_home.join("auth"))
}

pub fn moon_auth_store_path() -> Result<PathBuf> {
    Ok(moon_auth_dir()?.join(MOON_AUTH_FILE_NAME))
}

fn moon_auth_lock_path() -> Result<PathBuf> {
    Ok(moon_auth_dir()?.join(MOON_AUTH_LOCK_NAME))
}

fn codex_home_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("HOME directory could not be resolved"))?;
    let configured = match env_non_empty("CODEX_HOME") {
        Some(value) => value,
        None => return Ok(home.join(".codex")),
    };
    if configured == "~" {
        return Ok(home);
    }
    if let Some(rest) = configured.strip_prefix("~/") {
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(configured))
}

fn codex_cli_auth_store_path() -> Result<PathBuf> {
    Ok(codex_home_dir()?.join("auth.json"))
}

fn read_credential_from_path(
    path: &Path,
    source: CredentialSource,
) -> Result<Option<OpenAiCodexCredential>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let record: CodexAuthRecord = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if let Some(mode) = record.auth_mode.as_deref()
        && !mode.eq_ignore_ascii_case("chatgpt")
    {
        return Ok(None);
    }

    let Some(access_token) = record
        .tokens
        .access_token
        .as_deref()
        .and_then(trim_non_empty)
    else {
        return Ok(None);
    };
    let refresh_token = record
        .tokens
        .refresh_token
        .as_deref()
        .and_then(trim_non_empty);
    let id_token = record.tokens.id_token.as_deref().and_then(trim_non_empty);
    let account_id = record
        .tokens
        .account_id
        .as_deref()
        .and_then(trim_non_empty)
        .or_else(|| decode_access_token_account_id(&access_token));
    let email = decode_access_token_email(&access_token);
    let expires_at_epoch_ms = decode_access_token_expiry_ms(&access_token)
        .unwrap_or_else(|| fallback_expiry_ms(path, record.last_refresh.as_deref()));

    Ok(Some(OpenAiCodexCredential {
        access_token,
        refresh_token,
        id_token,
        account_id,
        email,
        expires_at_epoch_ms,
        source,
    }))
}

fn load_moon_credential() -> Result<Option<OpenAiCodexCredential>> {
    let path = moon_auth_store_path()?;
    read_credential_from_path(&path, CredentialSource::MoonStore)
}

fn load_external_codex_credential() -> Result<Option<OpenAiCodexCredential>> {
    let path = codex_cli_auth_store_path()?;
    read_credential_from_path(&path, CredentialSource::CodexCliStore)
}

fn persist_moon_credential(credential: &OpenAiCodexCredential) -> Result<PathBuf> {
    let path = moon_auth_store_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve moon auth directory"))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let record = CodexAuthRecord {
        auth_mode: Some("chatgpt".to_string()),
        last_refresh: Some(Utc::now().to_rfc3339()),
        tokens: CodexAuthTokens {
            access_token: Some(credential.access_token.clone()),
            refresh_token: credential.refresh_token.clone(),
            id_token: credential.id_token.clone(),
            account_id: credential.account_id.clone(),
        },
    };
    let payload = serde_json::to_string_pretty(&record)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, payload)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "failed to move {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;
    Ok(path)
}

fn open_auth_lock() -> Result<fs::File> {
    let lock_path = moon_auth_lock_path()?;
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    Ok(file)
}

fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(TOKEN_REQUEST_TIMEOUT_SECS))
        .build()
        .context("failed to build OAuth client")
}

fn parse_oauth_error_body(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if let Ok(parsed) = serde_json::from_str::<OAuthTokenResponse>(body)
        && let Some(error) = parsed.error
    {
        let detail = parsed
            .error_description
            .unwrap_or_else(|| "OpenAI OAuth request failed".to_string());
        return anyhow!("OpenAI OAuth request failed with status {status}: {error} ({detail})");
    }
    anyhow!(
        "OpenAI OAuth request failed with status {status}: {}",
        body.trim()
    )
}

fn exchange_oauth_form(form: &[(&str, String)]) -> Result<OAuthTokenResponse> {
    let client = build_client()?;
    let url = oauth_token_url()?;
    let response = client.post(url).form(form).send()?;
    let status = response.status();
    let body = response
        .text()
        .context("failed to read OAuth response body")?;
    if !status.is_success() {
        return Err(parse_oauth_error_body(status, &body));
    }
    let parsed: OAuthTokenResponse =
        serde_json::from_str(&body).context("failed to parse OAuth token response")?;
    if parsed.access_token.trim().is_empty() {
        return Err(anyhow!("OAuth response did not include an access token"));
    }
    Ok(parsed)
}

fn refresh_managed_credential(credential: OpenAiCodexCredential) -> Result<OpenAiCodexCredential> {
    let _lock = open_auth_lock()?;
    let Some(freshest) = load_moon_credential()? else {
        return Err(anyhow!("managed OpenAI OAuth credentials are missing"));
    };
    if !credential_is_expired(&freshest) {
        return Ok(freshest);
    }
    let refresh_token = freshest
        .refresh_token
        .as_deref()
        .and_then(trim_non_empty)
        .ok_or_else(|| anyhow!("managed OpenAI OAuth credentials are missing a refresh token"))?;

    let refreshed = exchange_oauth_form(&[
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.clone()),
        ("client_id", oauth_client_id()),
    ])?;
    let next_access = refreshed.access_token;
    let next_refresh = refreshed
        .refresh_token
        .or_else(|| freshest.refresh_token.clone());
    let next_id_token = refreshed.id_token.or_else(|| freshest.id_token.clone());
    let expires_at_epoch_ms = decode_access_token_expiry_ms(&next_access)
        .or_else(|| {
            refreshed
                .expires_in
                .map(|value| current_epoch_ms().saturating_add(value.saturating_mul(1_000)))
        })
        .unwrap_or_else(|| current_epoch_ms().saturating_add(60 * 60 * 1_000));
    let next = OpenAiCodexCredential {
        access_token: next_access,
        refresh_token: next_refresh,
        id_token: next_id_token,
        account_id: decode_access_token_account_id(&freshest.access_token)
            .or_else(|| freshest.account_id.clone())
            .or_else(|| decode_access_token_account_id(&credential.access_token)),
        email: decode_access_token_email(&freshest.access_token)
            .or_else(|| freshest.email.clone())
            .or_else(|| decode_access_token_email(&credential.access_token)),
        expires_at_epoch_ms,
        source: freshest.source.clone(),
    };
    persist_moon_credential(&next)?;
    Ok(next)
}

pub fn has_available_auth() -> bool {
    if env_non_empty("OPENAI_OAUTH_TOKEN").is_some() {
        return true;
    }
    match load_moon_credential() {
        Ok(Some(credential))
            if credential.refresh_token.is_some() || !credential_is_expired(&credential) =>
        {
            return true;
        }
        _ => {}
    }
    matches!(
        load_external_codex_credential(),
        Ok(Some(credential)) if !credential_is_expired(&credential)
    )
}

pub fn resolve_bearer_token() -> Result<Option<String>> {
    if let Some(token) = env_non_empty("OPENAI_OAUTH_TOKEN") {
        return Ok(Some(token));
    }

    if let Some(credential) = load_moon_credential()? {
        let managed = if credential_is_expired(&credential) {
            refresh_managed_credential(credential)?
        } else {
            credential
        };
        return Ok(Some(managed.access_token));
    }

    if let Some(credential) = load_external_codex_credential()?
        && !credential_is_expired(&credential)
    {
        return Ok(Some(credential.access_token));
    }

    Ok(None)
}

fn random_base64url(bytes: usize) -> String {
    let mut data = vec![0u8; bytes];
    OsRng.fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        cmd
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        cmd
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    };

    let status = command.status().context("failed to start browser opener")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("browser opener exited with status {status}"))
    }
}

fn read_http_request_path(stream: &mut std::net::TcpStream) -> Result<String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("failed to set callback read timeout")?;
    let mut buffer = [0u8; 8 * 1024];
    let size = stream
        .read(&mut buffer)
        .context("failed to read callback request")?;
    let request = String::from_utf8_lossy(&buffer[..size]);
    let request_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow!("callback request was empty"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method != "GET" || path.is_empty() {
        return Err(anyhow!("unexpected callback request line: {request_line}"));
    }
    Ok(path.to_string())
}

fn write_callback_response(
    stream: &mut std::net::TcpStream,
    status_line: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "{status_line}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .context("failed to write callback response")
}

fn parse_code_from_input(input: &str, expected_state: Option<&str>) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("authorization code input was empty"));
    }

    let maybe_url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Some(trimmed.to_string())
    } else if trimmed.starts_with("code=") || trimmed.contains("&code=") {
        Some(format!("http://localhost/auth/callback?{trimmed}"))
    } else {
        None
    };

    let Some(raw_url) = maybe_url else {
        return Ok(trimmed.to_string());
    };
    let url = Url::parse(&raw_url).context("failed to parse redirect URL")?;
    if let Some(error) = url
        .query_pairs()
        .find_map(|(key, value)| (key == "error").then(|| value.to_string()))
    {
        let detail = url
            .query_pairs()
            .find_map(|(key, value)| (key == "error_description").then(|| value.to_string()))
            .unwrap_or_else(|| "OpenAI OAuth returned an error".to_string());
        return Err(anyhow!("OpenAI OAuth returned {error}: {detail}"));
    }
    if let Some(expected_state) = expected_state {
        let actual_state = url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.to_string()));
        if actual_state.as_deref() != Some(expected_state) {
            return Err(anyhow!("OAuth state mismatch in callback"));
        }
    }
    url.query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.to_string()))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("callback URL did not include an authorization code"))
}

fn prompt_for_manual_code(authorize_url: &str) -> Result<String> {
    println!("Open this URL in your browser to authenticate:\n{authorize_url}\n");
    print!("Paste the authorization code (or full redirect URL): ");
    io::stdout().flush().context("failed to flush stdout")?;
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .context("failed to read authorization code from stdin")?;
    parse_code_from_input(&buffer, None)
}

fn wait_for_browser_callback(
    authorize_url: &str,
    expected_state: &str,
    browser_opened: bool,
) -> Result<String> {
    let listener = TcpListener::bind(oauth_callback_bind_addr()).with_context(|| {
        format!(
            "failed to bind OAuth callback listener on {}",
            oauth_callback_bind_addr()
        )
    })?;
    listener
        .set_nonblocking(true)
        .context("failed to set callback listener nonblocking mode")?;

    if browser_opened {
        println!("Completing OpenAI login in your browser.");
    } else {
        println!("Open this URL in your browser to authenticate:\n{authorize_url}\n");
    }
    println!(
        "Waiting for the browser callback at {}.",
        oauth_redirect_uri()
    );

    let deadline = Instant::now() + Duration::from_secs(OAUTH_LOGIN_TIMEOUT_SECS);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let path = read_http_request_path(&mut stream)?;
                let callback_url = format!("http://localhost{path}");
                match parse_code_from_input(&callback_url, Some(expected_state)) {
                    Ok(code) => {
                        write_callback_response(
                            &mut stream,
                            "HTTP/1.1 200 OK",
                            "<html><body><h1>Moon login complete</h1><p>You can return to the terminal.</p></body></html>",
                        )?;
                        return Ok(code);
                    }
                    Err(err) => {
                        let _ = write_callback_response(
                            &mut stream,
                            "HTTP/1.1 400 Bad Request",
                            "<html><body><h1>Moon login failed</h1><p>Return to the terminal for details.</p></body></html>",
                        );
                        return Err(err);
                    }
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for the OAuth callback; rerun `moon login --headless` to paste the redirect URL manually"
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(err).context("failed while waiting for OAuth callback"),
        }
    }
}

fn build_credential_from_token_response(
    response: OAuthTokenResponse,
    fallback_refresh_token: Option<String>,
) -> OpenAiCodexCredential {
    let access_token = response.access_token;
    let refresh_token = response.refresh_token.or(fallback_refresh_token);
    let expires_at_epoch_ms = decode_access_token_expiry_ms(&access_token)
        .or_else(|| {
            response
                .expires_in
                .map(|value| current_epoch_ms().saturating_add(value.saturating_mul(1_000)))
        })
        .unwrap_or_else(|| current_epoch_ms().saturating_add(60 * 60 * 1_000));
    OpenAiCodexCredential {
        access_token: access_token.clone(),
        refresh_token,
        id_token: response.id_token,
        account_id: decode_access_token_account_id(&access_token),
        email: decode_access_token_email(&access_token),
        expires_at_epoch_ms,
        source: CredentialSource::MoonStore,
    }
}

pub fn login(headless: bool) -> Result<LoginResult> {
    let verifier = random_base64url(32);
    let state = random_base64url(24);
    let mut authorize_url = oauth_authorize_url()?;
    authorize_url
        .query_pairs_mut()
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    let authorize_url = authorize_url.to_string();
    let mut browser_opened = false;
    let (code, callback_mode) = if headless {
        (
            prompt_for_manual_code(&authorize_url)?,
            LoginCallbackMode::ManualCode,
        )
    } else {
        browser_opened = open_browser(&authorize_url).is_ok();
        match wait_for_browser_callback(&authorize_url, &state, browser_opened) {
            Ok(code) => (code, LoginCallbackMode::BrowserCallback),
            Err(err)
                if err
                    .to_string()
                    .contains("failed to bind OAuth callback listener") =>
            {
                eprintln!("Local callback listener unavailable. Falling back to manual input.");
                (
                    prompt_for_manual_code(&authorize_url)?,
                    LoginCallbackMode::ManualCode,
                )
            }
            Err(err) => return Err(err),
        }
    };

    let token_response = exchange_oauth_form(&[
        ("grant_type", "authorization_code".to_string()),
        ("client_id", oauth_client_id()),
        ("redirect_uri", oauth_redirect_uri()),
        ("code_verifier", verifier),
        ("code", code),
    ])?;

    let credential = build_credential_from_token_response(token_response, None);
    let auth_store_path = persist_moon_credential(&credential)?;
    Ok(LoginResult {
        auth_store_path,
        account_id: credential.account_id,
        email: credential.email,
        expires_at_epoch_ms: credential.expires_at_epoch_ms,
        browser_opened,
        callback_mode,
    })
}
