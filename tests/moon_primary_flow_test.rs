#![cfg(not(windows))]
use predicates::str::contains;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use tempfile::tempdir;

fn write_executable(path: &Path, script: &str) {
    fs::write(path, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn start_fake_openai_compatible_server(response_body: &str) -> (thread::JoinHandle<()>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    let body = response_body.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .expect("read timeout");
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (handle, format!("http://{}", addr))
}

fn write_moon_env(moon_home: &Path) {
    fs::create_dir_all(moon_home).expect("mkdir moon env root");
    fs::write(moon_home.join(".env"), "\n").expect("write moon .env");
}

fn write_fake_qmd_collection_lifecycle(bin_path: &Path, log_path: &Path) {
    write_executable(
        bin_path,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

if [[ "${{1:-}}" == "collection" ]]; then
  case "${{2:-}}" in
    --help)
      echo "Commands: add remove show"
      exit 0
      ;;
    add|remove|show) exit 0 ;;
  esac
fi

exit 0
"#,
            log_path.display()
        ),
    );
}

#[test]
fn moon_record_captures_manifest_selected_session_into_raw() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    let sessions_dir = fs::canonicalize(&sessions_dir).expect("canonicalize sessions");
    write_moon_env(&moon_home);

    fs::write(
        sessions_dir.join("older.jsonl"),
        "{\"message\":{\"role\":\"user\"}}\n",
    )
    .expect("write old session");
    let latest_raw = "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"latest context\"}]}}\n";
    fs::write(sessions_dir.join("latest.jsonl"), latest_raw).expect("write latest session");
    fs::write(
        sessions_dir.join("sessions.json"),
        serde_json::to_string_pretty(&json!({
            "agent:main:one": {
                "sessionId": "older",
                "updatedAt": 10
            },
            "agent:main:two": {
                "sessionId": "latest",
                "updatedAt": 99
            }
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("OPENCLAW_SESSIONS_DIR", &sessions_dir)
        .arg("record")
        .assert()
        .success()
        .stdout(contains("record.session_id=latest"))
        .stdout(contains(format!(
            "record.target_path={}",
            moon_home.join("raw/latest.jsonl").display()
        )));

    let recorded = fs::read_to_string(moon_home.join("raw/latest.jsonl")).expect("read raw copy");
    assert_eq!(recorded, latest_raw);
}

#[test]
fn moon_project_writes_projection_markdown_into_mds() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let raw_dir = moon_home.join("raw");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    write_moon_env(&moon_home);

    let source = raw_dir.join("s1.jsonl");
    let session = json!({
        "type": "session",
        "id": "s1",
        "timestamp": "2026-03-12T00:00:00Z"
    });
    let user = json!({
        "message": {
            "role": "user",
            "createdAt": "2026-03-12T00:00:05Z",
            "content": [{"type":"text","text":"Please capture the current architecture."}]
        }
    });
    let tool_call = json!({
        "message": {
            "role": "assistant",
            "createdAt": "2026-03-12T00:00:10Z",
            "content": [{
                "type": "toolCall",
                "name": "exec",
                "arguments": {
                    "command": "rg -n moon-context-engine docs"
                }
            }]
        }
    });
    let tool_result = json!({
        "message": {
            "role": "toolResult",
            "createdAt": "2026-03-12T00:00:12Z",
            "content": [{"type":"text","text":"mip-moonv1.md: moon-context-engine is primary"}]
        }
    });
    let assistant = json!({
        "message": {
            "role": "assistant",
            "createdAt": "2026-03-12T00:00:15Z",
            "content": [{"type":"text","text":"Moon v1 uses moon-context-engine as the primary controller."}]
        }
    });
    fs::write(
        &source,
        format!("{session}\n{user}\n{tool_call}\n{tool_result}\n{assistant}\n"),
    )
    .expect("write raw session");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .arg("project")
        .arg("--session-id")
        .arg("s1")
        .assert()
        .success()
        .stdout(contains("project.session_id=s1"))
        .stdout(contains(format!(
            "project.target_path={}",
            moon_home.join("mds/history_hot_s1/session.md").display()
        )));

    let projection = fs::read_to_string(moon_home.join("mds/history_hot_s1/session.md"))
        .expect("read projection");
    assert!(projection.contains("moon_projection: 1"));
    assert!(projection.contains("## Conversations"));
    assert!(projection.contains("### User Queries"));
    assert!(projection.contains("Please capture the current architecture."));
    assert!(projection.contains("### Assistant Responses"));
    assert!(projection.contains("Moon v1 uses moon-context-engine as the primary controller."));
    assert!(projection.contains("## Tool Activity"));
    assert!(projection.contains("### exec"));
    assert!(projection.contains("mip-moonv1.md: moon-context-engine is primary"));
}

#[test]
fn moon_cleanse_writes_llm_compaction_summary() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let raw_dir = moon_home.join("raw");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);

    let source = moon_home.join("raw/session-a.jsonl");
    let user = json!({
        "message": {
            "role": "user",
            "createdAt": "2026-03-12T00:00:05Z",
            "content": [{"type":"text","text":"Please refactor cleanse so project owns raw to mds."}]
        }
    });
    let assistant = json!({
        "message": {
            "role": "assistant",
            "createdAt": "2026-03-12T00:00:15Z",
            "content": [{"type":"text","text":"I will split project from true LLM compaction."}]
        }
    });
    fs::write(&source, format!("{user}\n{assistant}\n")).expect("write raw");

    let response_body = r##"{"choices":[{"message":{"content":"# Cleanse Summary\n## Current Goal\n- Keep `project` responsible for raw to mds conversion.\n## Active Context\n- The runtime flow needs a distinct LLM compaction stage.\n## Decisions\n- Reserve `cleanse` for context compaction.\n## Open Tasks\n- Implement assemble later.\n## Risks / Blockers\n- Avoid mixing projection with compaction.\n## Relevant Evidence\n- The user explicitly separated project from cleanse."}}]}"##;
    let (server_handle, base_url) = start_fake_openai_compatible_server(response_body);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("MOON_CLEANSE_PROVIDER", "openai-compatible")
        .env("MOON_CLEANSE_MODEL", "test-cleanse")
        .env("AI_BASE_URL", &base_url)
        .env("AI_API_KEY", "test-key")
        .arg("cleanse")
        .arg("--session-id")
        .arg("session-a")
        .assert()
        .success()
        .stdout(contains("cleanse.provider=openai-compatible"))
        .stdout(contains(format!(
            "cleanse.summary_path={}",
            moon_home.join("cleanse/session-a.md").display()
        )));

    server_handle.join().expect("join fake server");

    let summary =
        fs::read_to_string(moon_home.join("cleanse/session-a.md")).expect("read cleanse summary");
    assert!(summary.contains("moon_cleanse: 1"));
    assert!(summary.contains("# Cleanse Summary"));
    assert!(summary.contains("Reserve `cleanse` for context compaction."));
    assert!(
        !moon_home
            .join("mds/history_hot_session-a/session.md")
            .exists(),
        "cleanse should not project into mds"
    );
}

#[test]
fn moon_assemble_writes_context_from_raw_and_cleanse_summary() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let raw_dir = moon_home.join("raw");
    let cleanse_dir = moon_home.join("cleanse");
    fs::create_dir_all(&raw_dir).expect("mkdir raw");
    fs::create_dir_all(&cleanse_dir).expect("mkdir cleanse");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);

    let source = raw_dir.join("session-b.jsonl");
    let user = json!({
        "message": {
            "role": "user",
            "createdAt": "2026-03-12T00:00:05Z",
            "content": [{"type":"text","text":"Prepare the next active context window."}]
        }
    });
    let assistant = json!({
        "message": {
            "role": "assistant",
            "createdAt": "2026-03-12T00:00:15Z",
            "content": [{"type":"text","text":"The assembly stage should include the latest cleanse summary."}]
        }
    });
    fs::write(&source, format!("{user}\n{assistant}\n")).expect("write raw");
    fs::write(
        cleanse_dir.join("session-b.md"),
        r#"---
moon_cleanse: 1
session_id: "session-b"
source_path: "/tmp/session-b.jsonl"
provider: "test"
model: "test-cleanse"
created_at_epoch_secs: 123
---

# Cleanse Summary
## Decisions
- Keep assembly as the pre-dispatch boundary.
"#,
    )
    .expect("write cleanse summary");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .arg("assemble")
        .arg("--session-id")
        .arg("session-b")
        .assert()
        .success()
        .stdout(contains("assemble.session_id=session-b"))
        .stdout(contains(format!(
            "assemble.output_path={}",
            moon_home.join("mce/session-b.md").display()
        )));

    let assembly =
        fs::read_to_string(moon_home.join("mce/session-b.md")).expect("read assembly output");
    assert!(assembly.contains("moon_assemble: 1"));
    assert!(assembly.contains("Keep assembly as the pre-dispatch boundary."));
    assert!(assembly.contains("## Embedding Index Anchor"));
    assert!(assembly.contains("Prepare the next active context window."));
}

#[test]
fn moon_recall_falls_back_to_search_and_surfaces_hits() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);
    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_executable(
        &qmd,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{}"

if [[ "${{1:-}}" == "query" ]]; then
  echo "query unsupported" >&2
  exit 1
fi

if [[ "${{1:-}}" == "search" ]]; then
  cat <<'EOF'
{{"results":[{{"path":"/tmp/history/one.md","score":0.91,"snippet":"Decision: keep recall in Moon v1."}}]}}
EOF
  exit 0
fi

exit 1
"#,
            qmd_log.display()
        ),
    );

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("recall")
        .args(["--name", "history"])
        .args(["--query", "moon recall"])
        .args(["--limit", "3"])
        .assert()
        .success()
        .stdout(contains("recall.mode=search"))
        .stdout(contains("recall.result_count=1"))
        .stdout(contains("hit[1].source=/tmp/history/one.md"))
        .stdout(contains("hit[1].text=Decision: keep recall in Moon v1."));

    let log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(log.contains("query moon recall --json -c history -n 3"));
    assert!(log.contains("search moon recall --json -c history -n 3"));
}

#[test]
fn moon_distill_norm_dry_run_selects_pending_mlib_doc_without_archive_flag() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let mlib_dir = moon_home.join("mlib");
    fs::create_dir_all(&mlib_dir).expect("mkdir mlib");
    write_moon_env(&moon_home);

    let alpha = mlib_dir.join("alpha.md");
    let beta = mlib_dir.join("beta.md");
    fs::write(&alpha, "alpha").expect("write alpha");
    fs::write(&beta, "beta").expect("write beta");

    let state_dir = moon_home.join("state");
    fs::create_dir_all(&state_dir).expect("mkdir state");
    fs::write(
        state_dir.join("moon_state.json"),
        format!(
            r#"{{
  "schema_version": 3,
  "last_heartbeat_epoch_secs": 1,
  "distilled_archives": {{
    "{}": 4102444800
  }},
  "embedded_projections": {{}},
  "inbound_seen_files": {{}}
}}
"#,
            beta.display()
        ),
    )
    .expect("write state");

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(tmp.path())
        .env("MOON_HOME", &moon_home)
        .arg("distill")
        .args(["--mode", "norm"])
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(contains("distill.mode=norm"))
        .stdout(contains(format!("archive_path={}", alpha.display())));
}

#[test]
fn moon_context_engine_writes_assembly_output() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    let sessions_dir = tmp.path().join("sessions");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    fs::create_dir_all(&sessions_dir).expect("mkdir sessions");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    let sessions_dir = fs::canonicalize(&sessions_dir).expect("canonicalize sessions");
    write_moon_env(&moon_home);

    let source = sessions_dir.join("session-c.jsonl");
    let user = json!({
        "message": {
            "role": "user",
            "createdAt": "2026-03-12T00:00:05Z",
            "content": [{"type":"text","text":"Wire the new runtime entry."}]
        }
    });
    let assistant = json!({
        "message": {
            "role": "assistant",
            "createdAt": "2026-03-12T00:00:15Z",
            "content": [{"type":"text","text":"The dedicated entry should own record, cleanse, and assemble."}]
        }
    });
    fs::write(&source, format!("{user}\n{assistant}\n")).expect("write source");

    let response_body = r##"{"choices":[{"message":{"content":"# Cleanse Summary\n## Current Goal\n- Wire the dedicated runtime entry.\n## Decisions\n- Keep the primary path MOON-owned.\n## Open Tasks\n- Persist assembled context for the runtime entry.\n## Risks / Blockers\n- Do not route back through watcher-first ownership."}}]}"##;
    let (server_handle, base_url) = start_fake_openai_compatible_server(response_body);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("MOON_CLEANSE_PROVIDER", "openai-compatible")
        .env("MOON_CLEANSE_MODEL", "test-cleanse")
        .env("AI_BASE_URL", &base_url)
        .env("AI_API_KEY", "test-key")
        .arg("context-engine")
        .args(["--source", &source.display().to_string()])
        .args(["--session-id", "session-c"])
        .arg("--force-cleanse")
        .assert()
        .success()
        .stdout(contains("context_engine.session_id=session-c"))
        .stdout(contains(format!(
            "context_engine.assembly_path={}",
            moon_home.join("mce/session-c.md").display()
        )));

    server_handle.join().expect("join fake server");

    let assembly =
        fs::read_to_string(moon_home.join("mce/session-c.md")).expect("read assembly output");
    assert!(assembly.contains("moon_assemble: 1"));
    assert!(assembly.contains("Wire the dedicated runtime entry."));
    assert!(assembly.contains("## Embedding Index Anchor"));
    assert!(assembly.contains("Keep the primary path MOON-owned."));
    assert!(assembly.contains("Persist assembled context for the runtime entry."));
    assert!(
        moon_home
            .join("mds/history_hot_session-c/session.md")
            .exists()
    );
}

#[test]
fn moon_context_engine_rotates_hot_qmd_collections_on_session_switch() {
    let tmp = tempdir().expect("tempdir");
    let moon_home = tmp.path().join("moon");
    fs::create_dir_all(&moon_home).expect("mkdir moon");
    let moon_home = fs::canonicalize(&moon_home).expect("canonicalize moon");
    write_moon_env(&moon_home);

    let source_a = moon_home.join("session-hot-a.jsonl");
    fs::write(
        &source_a,
        "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"session a context\"}]}}\n",
    )
    .expect("write source a");
    let source_b = moon_home.join("session-hot-b.jsonl");
    fs::write(
        &source_b,
        "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"session b context\"}]}}\n",
    )
    .expect("write source b");

    let qmd = tmp.path().join("qmd");
    let qmd_log = tmp.path().join("qmd.log");
    write_fake_qmd_collection_lifecycle(&qmd, &qmd_log);

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_a.display().to_string()])
        .args(["--session-id", "s-hot-a"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success()
        .stdout(contains("context_engine.session_id=s-hot-a"));

    assert_cmd::cargo::cargo_bin_cmd!("moon")
        .current_dir(&moon_home)
        .env("MOON_HOME", &moon_home)
        .env("QMD_BIN", &qmd)
        .arg("context-engine")
        .args(["--source", &source_b.display().to_string()])
        .args(["--session-id", "s-hot-b"])
        .args(["--used-tokens", "1000"])
        .args(["--max-tokens", "200000"])
        .assert()
        .success()
        .stdout(contains("context_engine.session_id=s-hot-b"));

    let qmd_log = fs::read_to_string(&qmd_log).expect("read qmd log");
    assert!(qmd_log.contains("collection add"));
    assert!(qmd_log.contains("--name history_hot_s-hot-a"));
    assert!(qmd_log.contains("--name history_hot_s-hot-b"));
    assert!(qmd_log.contains("collection remove history_hot_s-hot-a"));

    let state_raw =
        fs::read_to_string(moon_home.join("state/moon_state.json")).expect("read state");
    let state: Value = serde_json::from_str(&state_raw).expect("parse state");
    let managed = state
        .get("managed_hot_collections")
        .and_then(Value::as_object)
        .expect("managed_hot_collections object");
    assert!(managed.contains_key("history_hot_s-hot-b"));
    assert!(!managed.contains_key("history_hot_s-hot-a"));
}
