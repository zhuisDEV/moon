#![cfg(not(windows))]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use predicates::str::contains;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

#[derive(Debug)]
struct CapturedRequest {
    path: String,
    raw: String,
    body: String,
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("epoch")
        .as_secs()
}

#[cfg(unix)]
fn assert_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(
        mode & 0o077,
        0,
        "expected owner-only permissions for {} but got {:03o}",
        path.display(),
        mode
    );
}

fn make_jwt(email: &str, account_id: &str, exp_epoch_secs: u64) -> String {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        json!({
            "exp": exp_epoch_secs,
            "https://api.openai.com/profile": {
                "email": email,
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
            }
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("read timeout");
    let mut buffer = [0u8; 16 * 1024];
    let mut bytes = Vec::new();
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(_) => break,
        }
    }
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .map(|(head, body)| (head.to_string(), body.to_string()))
        .unwrap_or_else(|| (raw.clone(), String::new()));
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    CapturedRequest { path, raw, body }
}

fn start_fake_json_server(body: String) -> (thread::JoinHandle<CapturedRequest>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake json server");
    let addr = listener.local_addr().expect("local addr");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let request = read_request(&mut stream);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
        request
    });
    (handle, format!("http://{}", addr))
}

fn start_fake_status_server(
    status_line: &str,
    headers: &[(&str, &str)],
    body: String,
) -> (thread::JoinHandle<CapturedRequest>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake status server");
    let addr = listener.local_addr().expect("local addr");
    let status = status_line.to_string();
    let response_headers: Vec<(String, String)> = headers
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let request = read_request(&mut stream);
        let mut response = format!("HTTP/1.1 {status}\r\n");
        for (key, value) in response_headers {
            response.push_str(&format!("{key}: {value}\r\n"));
        }
        response.push_str(&format!(
            "content-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        ));
        stream
            .write_all(response.as_bytes())
            .expect("write response");
        request
    });
    (handle, format!("http://{}", addr))
}

fn write_moon_env(moon_home: &Path) {
    fs::create_dir_all(moon_home).expect("mkdir moon home");
    fs::write(moon_home.join(".env"), "\n").expect("write moon env");
}

fn write_openai_codex_auth(
    moon_home: &Path,
    access_token: &str,
    refresh_token: &str,
    account_id: &str,
) {
    let auth_dir = moon_home.join("auth");
    fs::create_dir_all(&auth_dir).expect("mkdir auth dir");
    fs::write(
        auth_dir.join("openai-codex.json"),
        serde_json::to_string_pretty(&json!({
            "auth_mode": "chatgpt",
            "last_refresh": "2026-04-17T00:00:00Z",
            "tokens": {
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id,
            }
        }))
        .expect("serialize auth file"),
    )
    .expect("write auth file");
}

fn write_raw_session(moon_home: &Path, session_id: &str) {
    let raw_dir = moon_home.join("raw");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    fs::write(
        raw_dir.join(format!("{session_id}.jsonl")),
        "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Summarize the current session.\"}]}}\n",
    )
    .expect("write raw session");
}

#[test]
fn moon_login_headless_persists_managed_openai_codex_credentials() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    write_moon_env(&moon_home);

    let account_id = "acct-login-123";
    let access_token = make_jwt("login@example.com", account_id, now_epoch_secs() + 3600);
    let refresh_token = "refresh-login-token";
    let response_body = json!({
        "access_token": access_token,
        "refresh_token": refresh_token,
        "id_token": "id-token-login"
    })
    .to_string();
    let (auth_handle, auth_base_url) = start_fake_json_server(response_body);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_OPENAI_OAUTH_BASE_URL", &auth_base_url)
        .arg("login")
        .arg("--headless")
        .write_stdin("login-code-123\n")
        .assert()
        .success()
        .stdout(contains("login.provider=openai-codex"))
        .stdout(contains("login.callback_mode=manual"))
        .stdout(contains("login.browser_opened=false"))
        .stdout(contains("login.email=login@example.com"))
        .stdout(contains(format!("login.account_id={account_id}")));

    let captured = auth_handle.join().expect("join auth server");
    assert_eq!(captured.path, "/oauth/token");
    assert!(captured.body.contains("grant_type=authorization_code"));
    assert!(captured.body.contains("code=login-code-123"));
    assert!(captured.body.contains("code_verifier="));

    let auth_file: Value = serde_json::from_str(
        &fs::read_to_string(moon_home.join("auth/openai-codex.json")).expect("read auth file"),
    )
    .expect("parse auth file");
    assert_eq!(
        auth_file["tokens"]["access_token"].as_str(),
        Some(access_token.as_str())
    );
    assert_eq!(
        auth_file["tokens"]["refresh_token"].as_str(),
        Some(refresh_token)
    );
    assert_eq!(auth_file["tokens"]["account_id"].as_str(), Some(account_id));
    #[cfg(unix)]
    {
        assert_owner_only(&moon_home.join("auth"));
        assert_owner_only(&moon_home.join("auth/openai-codex.json"));
    }
}

#[test]
fn moon_cleanse_uses_managed_openai_codex_auth_store() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    write_moon_env(&moon_home);
    write_raw_session(&moon_home, "s-managed");

    let account_id = "acct-managed-1";
    let access_token = make_jwt("managed@example.com", account_id, now_epoch_secs() + 3600);
    write_openai_codex_auth(&moon_home, &access_token, "refresh-managed", account_id);

    let response_body = json!({
        "output_text": "# Cleanse Summary\n## Current Goal\n- Use managed OAuth.\n## Active Context\n- Managed token should be accepted.\n## Decisions\n- Prefer Moon auth store.\n## Open Tasks\n- None.\n## Risks / Blockers\n- None.\n## Relevant Evidence\n- Managed token loaded."
    })
    .to_string();
    let (responses_handle, base_url) = start_fake_json_server(response_body);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_CLEANSE_PROVIDER", "openai-codex")
        .env("MOON_CLEANSE_MODEL", "gpt-5.4")
        .env("OPENAI_CODEX_BASE_URL", &base_url)
        .arg("cleanse")
        .arg("--session-id")
        .arg("s-managed")
        .assert()
        .success()
        .stdout(contains("cleanse.provider=openai-codex"))
        .stdout(contains("cleanse.model=gpt-5.4"));

    let captured = responses_handle.join().expect("join responses server");
    assert_eq!(captured.path, "/codex/responses");
    assert!(captured.raw.contains(access_token.as_str()));
}

#[test]
fn moon_cleanse_refreshes_expired_managed_openai_codex_token() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    write_moon_env(&moon_home);
    write_raw_session(&moon_home, "s-refresh");

    let account_id = "acct-refresh-1";
    let expired_access_token = make_jwt(
        "refresh@example.com",
        account_id,
        now_epoch_secs().saturating_sub(60),
    );
    write_openai_codex_auth(
        &moon_home,
        &expired_access_token,
        "refresh-old-token",
        account_id,
    );

    let refreshed_access_token =
        make_jwt("refresh@example.com", account_id, now_epoch_secs() + 7200);
    let refreshed_response = json!({
        "access_token": refreshed_access_token,
        "refresh_token": "refresh-new-token",
    })
    .to_string();
    let (auth_handle, auth_base_url) = start_fake_json_server(refreshed_response);

    let responses_body = json!({
        "output_text": "# Cleanse Summary\n## Current Goal\n- Refresh before request.\n## Active Context\n- Managed auth is stale.\n## Decisions\n- Refresh token should rotate.\n## Open Tasks\n- None.\n## Risks / Blockers\n- None.\n## Relevant Evidence\n- Refresh path exercised."
    })
    .to_string();
    let (responses_handle, responses_base_url) = start_fake_json_server(responses_body);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_CLEANSE_PROVIDER", "openai-codex")
        .env("MOON_CLEANSE_MODEL", "gpt-5.4")
        .env("MOON_OPENAI_OAUTH_BASE_URL", &auth_base_url)
        .env("OPENAI_CODEX_BASE_URL", &responses_base_url)
        .arg("cleanse")
        .arg("--session-id")
        .arg("s-refresh")
        .assert()
        .success()
        .stdout(contains("cleanse.provider=openai-codex"));

    let auth_request = auth_handle.join().expect("join auth refresh server");
    assert_eq!(auth_request.path, "/oauth/token");
    assert!(auth_request.body.contains("grant_type=refresh_token"));
    assert!(
        auth_request
            .body
            .contains("refresh_token=refresh-old-token")
    );

    let responses_request = responses_handle.join().expect("join responses server");
    assert!(
        responses_request
            .raw
            .contains(refreshed_access_token.as_str())
    );

    let auth_file: Value = serde_json::from_str(
        &fs::read_to_string(moon_home.join("auth/openai-codex.json")).expect("read auth file"),
    )
    .expect("parse auth file");
    assert_eq!(
        auth_file["tokens"]["refresh_token"].as_str(),
        Some("refresh-new-token")
    );
    assert_eq!(
        auth_file["tokens"]["access_token"].as_str(),
        Some(refreshed_access_token.as_str())
    );
}

#[test]
fn moon_login_sanitizes_oauth_error_bodies() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    write_moon_env(&moon_home);

    let secret_body = "oauth-body-secret-token";
    let (auth_handle, auth_base_url) = start_fake_status_server(
        "401 Unauthorized",
        &[
            ("content-type", "text/plain"),
            ("x-request-id", "req-login-401"),
        ],
        secret_body.to_string(),
    );

    let assert = assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .env("MOON_OPENAI_OAUTH_BASE_URL", &auth_base_url)
        .arg("login")
        .arg("--headless")
        .write_stdin("login-code-401\n")
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).expect("utf8 stderr");
    assert!(
        stderr.contains(
            "OpenAI OAuth request failed with status 401 Unauthorized request_id=req-login-401"
        ),
        "stderr should report sanitized oauth failure: {stderr}"
    );
    assert!(
        !stdout.contains(secret_body) && !stderr.contains(secret_body),
        "raw oauth response body should not leak into command output"
    );

    let captured = auth_handle.join().expect("join auth server");
    assert_eq!(captured.path, "/oauth/token");
}
