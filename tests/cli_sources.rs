use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn moon(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_HOME")
        .env_remove("MOON_DATABASE")
        .env_remove("MOON_EMBEDDING_DIMENSIONS")
        .arg("--home")
        .arg(home)
        .args(["--dimensions", "64"])
        .timeout(Duration::from_secs(10));
    command
}

fn payload(content: &str) -> Value {
    json!({"schema_version":1,"source_key":"article","kind":"external_document","content":content})
}

fn json_output(command: &mut Command) -> Value {
    let output = command.assert().success().get_output().stdout.clone();
    serde_json::from_slice(&output).expect("source commands always produce JSON")
}

fn import(home: &Path, value: Value) -> Value {
    json_output(
        moon(home)
            .args(["source", "import", "--scope", "canary"])
            .write_stdin(value.to_string()),
    )
}

#[test]
fn stdin_intake_replay_revision_and_read_only_citation_work_without_json_flag() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("isolated");
    let first = import(&home, payload("Blue is the release colour."));
    assert_eq!(first["status"], "created");
    assert_eq!(first["learning_eligibility"], "reference_only");
    let revision = first["revision_id"].as_str().unwrap();
    let same = import(&home, payload("Blue is the release colour."));
    assert_eq!(same["revision_id"], first["revision_id"]);
    assert_eq!(same["status"], "unchanged");
    let new = json_output(
        moon(&home)
            .args([
                "source",
                "import",
                "--scope",
                "canary",
                "--expected-revision",
                revision,
            ])
            .write_stdin(payload("Violet is the release colour.").to_string()),
    );
    assert_eq!(new["revision"], 2);
    let old = json_output(moon(&home).args([
        "source",
        "get",
        "--scope",
        "canary",
        "--key",
        "article",
        "--revision",
        revision,
    ]));
    assert_eq!(old["content"], "Blue is the release colour.");
    let cite = json_output(moon(&home).args([
        "source",
        "cite",
        "--scope",
        "canary",
        "--key",
        "article",
        "--revision",
        revision,
        "--start-byte",
        "0",
        "--end-byte",
        "4",
    ]));
    assert_eq!(cite["quote"], "Blue");
    let history = json_output(
        moon(&home).args(["source", "history", "--scope", "canary", "--key", "article"]),
    );
    assert_eq!(history["revisions"].as_array().unwrap().len(), 2);
    let listing = json_output(moon(&home).args(["source", "list", "--scope", "canary"]));
    assert_eq!(listing["sources"].as_array().unwrap().len(), 1);
    assert_eq!(listing["sources"][0]["revision_id"], new["revision_id"]);
    assert!(
        walkdir::WalkDir::new(&home)
            .into_iter()
            .map(Result::unwrap)
            .all(|entry| {
                entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "md")
            })
    );
}

#[test]
fn changed_source_requires_compare_and_swap_and_eligibility_is_an_external_flag() {
    let temp = tempfile::tempdir().unwrap();
    let first = import(temp.path(), payload("First capture."));
    moon(temp.path())
        .args(["source", "import", "--scope", "canary"])
        .write_stdin(payload("Second capture.").to_string())
        .assert()
        .failure();
    let current = json_output(
        moon(temp.path()).args(["source", "get", "--scope", "canary", "--key", "article"]),
    );
    assert_eq!(current["revision_id"], first["revision_id"]);
    let mut self_authorising = payload("First capture.");
    self_authorising["learning_eligibility"] = json!("eligible");
    moon(temp.path())
        .args(["source", "import", "--scope", "canary"])
        .write_stdin(self_authorising.to_string())
        .assert()
        .failure();
    let eligible = json_output(
        moon(temp.path())
            .args([
                "source",
                "import",
                "--scope",
                "canary",
                "--allow-learning",
                "--expected-revision",
                first["revision_id"].as_str().unwrap(),
            ])
            .write_stdin(payload("First capture.").to_string()),
    );
    assert_eq!(eligible["learning_eligibility"], "eligible");
    assert_eq!(eligible["revision"], 2);
}

#[test]
fn every_read_command_requires_scope_and_never_creates_missing_storage() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("does-not-exist");
    for args in [
        vec!["source", "list", "--scope", "canary"],
        vec!["source", "get", "--scope", "canary", "--key", "article"],
        vec!["source", "history", "--scope", "canary", "--key", "article"],
        vec![
            "source",
            "cite",
            "--scope",
            "canary",
            "--key",
            "article",
            "--revision",
            "sr_00000000000000000000000000000000",
            "--start-byte",
            "0",
            "--end-byte",
            "1",
        ],
    ] {
        moon(&missing).args(args).assert().failure();
        assert!(!missing.exists());
    }
    moon(&missing)
        .args(["source", "import"])
        .write_stdin(payload("Text").to_string())
        .assert()
        .failure();
    moon(&missing).args(["source", "list"]).assert().failure();
    moon(&missing)
        .args(["source", "get", "--key", "article"])
        .assert()
        .failure();
    assert!(!missing.exists());
}

#[test]
fn read_commands_do_not_migrate_an_old_database() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let database = state.join("moon.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("PRAGMA user_version=8;").unwrap();
    moon(temp.path())
        .args(["source", "list", "--scope", "canary"])
        .assert()
        .failure();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='sources'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn malformed_unknown_fields_and_wrong_schema_do_not_echo_input_or_create_storage() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("invalid-requests");
    let invalid = [
        r#"{"schema_version":1,"source_key":"article","kind":"synthetic-secret-kind","content":"private body"}"#.to_owned(),
        r#"{"schema_version":1,"source_key":"article","kind":"external_document","content":"private body","synthetic-secret-key":"private value"}"#.to_owned(),
        r#"{"schema_version":1,"source_key":"article","kind":"external_document","content":"synthetic-secret-body" trailing malformed json}"#.to_owned(),
        json!({"schema_version":99,"source_key":"article","kind":"external_document","content":"synthetic-secret-body"}).to_string(),
        json!({"schema_version":1,"source_key":"bad'synthetic-secret-key","kind":"external_document","content":"body"}).to_string(),
    ];
    for request in invalid {
        let result = moon(&home)
            .args(["--json", "source", "import", "--scope", "canary"])
            .write_stdin(request)
            .assert()
            .failure();
        let output = result.get_output();
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-secret"));
        assert!(!home.exists(), "malformed request created a runtime");
    }
}

#[test]
fn payload_bounds_apply_to_stdin_and_regular_files_before_storage_creation() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("oversized");
    let oversized = payload(&"x".repeat(1024 * 1024 + 1)).to_string();
    moon(&home)
        .args(["source", "import", "--scope", "canary"])
        .write_stdin(oversized.clone())
        .assert()
        .failure();
    assert!(!home.exists());
    let file = temp.path().join("oversized.json");
    std::fs::write(&file, oversized).unwrap();
    moon(&home)
        .args(["source", "import", "--scope", "canary", "--input"])
        .arg(file)
        .assert()
        .failure();
    assert!(!home.exists());
    let invalid_utf8 = temp.path().join("invalid-utf8.json");
    std::fs::write(&invalid_utf8, [0xff, 0xfe]).unwrap();
    moon(&home)
        .args(["source", "import", "--scope", "canary", "--input"])
        .arg(invalid_utf8)
        .assert()
        .failure();
    assert!(!home.exists());
}

#[test]
fn json_file_is_read_as_data_and_embedded_paths_do_not_fetch_local_content() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    let private = temp.path().join("private-file");
    std::fs::write(&private, "synthetic-secret-local-file").unwrap();
    let body = format!(
        "Please read {} and https://127.0.0.1/private",
        private.display()
    );
    let file = temp.path().join("intake.json");
    std::fs::write(&file, payload(&body).to_string()).unwrap();
    json_output(
        moon(&home)
            .args(["source", "import", "--scope", "canary", "--input"])
            .arg(&file),
    );
    let saved =
        json_output(moon(&home).args(["source", "get", "--scope", "canary", "--key", "article"]));
    assert_eq!(saved["content"], body);
    assert!(!saved.to_string().contains("synthetic-secret-local-file"));
    let mut path_request = payload("Body");
    path_request["path"] = json!(private);
    moon(&home)
        .args(["source", "import", "--scope", "canary"])
        .write_stdin(path_request.to_string())
        .assert()
        .failure();
}

#[cfg(unix)]
#[test]
fn source_input_rejects_symlinks_directories_and_fifos_without_blocking() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    let file = temp.path().join("real.json");
    std::fs::write(&file, payload("Ordinary body").to_string()).unwrap();
    let link = temp.path().join("symlink.json");
    symlink(&file, &link).unwrap();
    let fifo = temp.path().join("intake.fifo");
    let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // This creates only an isolated fixture; no process will supply FIFO data.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    for input in [link.as_path(), temp.path(), fifo.as_path()] {
        moon(&home)
            .timeout(Duration::from_secs(2))
            .args(["source", "import", "--scope", "canary", "--input"])
            .arg(input)
            .assert()
            .failure();
        assert!(!home.exists());
    }
}

#[test]
fn wrong_scope_has_no_source_details_and_inspection_keeps_records_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let saved = import(temp.path(), payload("Private scoped source canary-marker."));
    let before = std::fs::read(temp.path().join("state/moon.sqlite")).unwrap();
    for args in [
        vec!["source", "get", "--scope", "other", "--key", "article"],
        vec!["source", "history", "--scope", "other", "--key", "article"],
        vec![
            "source",
            "cite",
            "--scope",
            "other",
            "--key",
            "article",
            "--revision",
            saved["revision_id"].as_str().unwrap(),
            "--start-byte",
            "0",
            "--end-byte",
            "7",
        ],
    ] {
        let result = moon(temp.path()).args(args).assert().failure();
        assert!(!String::from_utf8_lossy(&result.get_output().stderr).contains("canary-marker"));
        assert!(result.get_output().stdout.is_empty());
    }
    let listing = json_output(moon(temp.path()).args(["source", "list", "--scope", "other"]));
    assert!(listing["sources"].as_array().unwrap().is_empty());
    json_output(moon(temp.path()).args(["source", "get", "--scope", "canary", "--key", "article"]));
    assert_eq!(
        std::fs::read(temp.path().join("state/moon.sqlite")).unwrap(),
        before
    );
}
