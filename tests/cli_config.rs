use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn moon(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_DATABASE")
        .env_remove("MOON_HOME")
        .env_remove("MOON_EMBEDDING_DIMENSIONS")
        .arg("--home")
        .arg(home)
        .arg("--json")
        .arg("config");
    command
}

fn output(home: &Path, action: &str) -> Value {
    let result = moon(home)
        .arg(action)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&result).expect("valid config JSON")
}

#[test]
fn absent_config_preserves_legacy_defaults_and_does_not_create_a_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("missing");
    let shown = output(&home, "show");
    assert_eq!(shown["path"], home.join("moon.toml").to_str().unwrap());
    assert_eq!(shown["present"], false);
    assert_eq!(shown["learning"]["observation_ttl_hours"], 24);
    assert_eq!(shown["learning"]["l1"]["enabled"], true);
    assert_eq!(shown["learning"]["l2"]["enabled"], false);
    for stage in ["l1", "l2"] {
        for field in [
            "model",
            "reasoning",
            "fallback_model",
            "fallback_reasoning",
            "fallback_enabled",
            "timeout_ms",
            "max_output_tokens",
            "prompt_file",
        ] {
            assert!(shown["learning"][stage][field].is_null(), "{stage}.{field}");
        }
    }
    assert_eq!(shown["learning"]["l2"]["daily_at"], "03:00");
    assert_eq!(shown["learning"]["l2"]["timezone"], "Australia/Sydney");
    assert_eq!(shown["learning"]["l2"]["batch_size"], 32);
    assert_eq!(shown["learning"]["l2"]["max_batches_per_day"], 8);
    assert_eq!(shown["learning"]["l2"]["max_actions"], 16);
    assert_eq!(shown["learning"]["l2"]["max_input_chars"], 64_000);
    assert_eq!(output(&home, "validate"), shown);
    assert!(!home.exists());
}

#[test]
fn independent_stages_and_prompt_paths_resolve_from_selected_home() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    fs::create_dir(&home).unwrap();
    let absolute_prompt = temp.path().join("absolute-l2.md");
    fs::create_dir(home.join("prompts")).unwrap();
    fs::write(home.join("prompts/l1.md"), "L1 test prompt.").unwrap();
    fs::write(&absolute_prompt, "L2 test prompt.").unwrap();
    let config = format!(
        r#"
[learning]
observation_ttl_hours = 12
[learning.l1]
enabled = false
model = "openai/gpt-6-astra"
reasoning = "low"
fallback_model = "other/small"
fallback_reasoning = "off"
fallback_enabled = false
timeout_ms = 15000
max_output_tokens = 2048
prompt_file = "prompts/l1.md"
[learning.l2]
enabled = true
model = "another/large"
reasoning = "xhigh"
fallback_model = "other/large"
fallback_reasoning = "high"
fallback_enabled = true
timeout_ms = 600000
max_output_tokens = 32768
daily_at = "23:59"
timezone = "Etc/GMT+5"
batch_size = 64
max_batches_per_day = 4
max_actions = 8
max_input_chars = 32000
prompt_file = "{}"
"#,
        absolute_prompt.display()
    );
    fs::write(home.join("moon.toml"), config).unwrap();
    let shown = output(&home, "show");
    assert_eq!(shown["present"], true);
    assert_eq!(shown["learning"]["observation_ttl_hours"], 12);
    let l1 = &shown["learning"]["l1"];
    assert_eq!(l1["enabled"], false);
    assert_eq!(l1["model"], "openai/gpt-6-astra");
    assert_eq!(l1["reasoning"], "low");
    assert_eq!(l1["fallback_model"], "other/small");
    assert_eq!(l1["fallback_reasoning"], "off");
    assert_eq!(l1["fallback_enabled"], false);
    assert_eq!(l1["timeout_ms"], 15_000);
    assert_eq!(l1["max_output_tokens"], 2_048);
    assert_eq!(
        l1["prompt_file"],
        home.join("prompts/l1.md").to_str().unwrap()
    );
    let l2 = &shown["learning"]["l2"];
    assert_eq!(l2["model"], "another/large");
    assert_eq!(l2["reasoning"], "xhigh");
    assert_eq!(l2["fallback_model"], "other/large");
    assert_eq!(l2["fallback_reasoning"], "high");
    assert_eq!(l2["fallback_enabled"], true);
    assert_eq!(l2["timeout_ms"], 600_000);
    assert_eq!(l2["max_output_tokens"], 32_768);
    assert_eq!(l2["prompt_file"], absolute_prompt.to_str().unwrap());
    assert_eq!(output(&home, "validate"), shown);
    assert!(!home.join("state").exists());
}

#[test]
fn explicit_home_wins_over_ambient_home_and_database() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    let ambient = temp.path().join("ambient");
    fs::create_dir(&ambient).unwrap();
    fs::write(ambient.join("moon.toml"), "not valid TOML").unwrap();
    let ambient_database = ambient.join("must-not-be-created.sqlite");
    moon(&selected)
        .env("MOON_HOME", &ambient)
        .env("MOON_DATABASE", &ambient_database)
        .arg("show")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""present":false"#));
    assert!(!selected.exists());
    assert!(!ambient_database.exists());
}

#[test]
fn relative_home_is_reported_as_absolute() {
    let temp = tempfile::tempdir().unwrap();
    let result = moon(Path::new("relative-moon"))
        .current_dir(temp.path())
        .arg("show")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(
        shown["path"],
        temp.path()
            .canonicalize()
            .unwrap()
            .join("relative-moon/moon.toml")
            .to_str()
            .unwrap()
    );
    assert!(!temp.path().join("relative-moon").exists());
}

#[test]
fn starter_is_private_valid_and_never_overwrites_existing_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("new-runtime");
    let created = output(&home, "init");
    assert_eq!(created["present"], true);
    assert_eq!(created["learning"]["l1"]["model"], "openai/gpt-6-astra");
    assert_eq!(created["learning"]["l1"]["reasoning"], "low");
    assert_eq!(created["learning"]["l2"]["model"], "openai/gpt-6-astra");
    assert_eq!(created["learning"]["l2"]["reasoning"], "xhigh");
    assert_eq!(created["learning"]["l2"]["enabled"], false);
    let path = home.join("moon.toml");
    let original = fs::read(&path).unwrap();
    moon(&home)
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(output(&home, "validate"), created);
    assert!(!home.join("state").exists());
    assert_eq!(fs::read_dir(&home).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn unknown_fields_and_wrong_types_fail_without_disclosing_source_text() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    for source in [
        "top_level_private_marker = 'never-print-this-secret'",
        "[learning]\napi_key = 'never-print-this-secret'",
        "[learning.l1]\nprivate_marker = 'never-print-this-secret'",
        "[learning.l2]\nprivate_marker = 'never-print-this-secret'",
        "[learning.l1]\ndaily_at = 'never-print-this-secret'",
        "[learning.l1]\nenabled = 'never-print-this-secret'",
        "[learning.l2]\ntimeout_ms = 'never-print-this-secret'",
        "[learning.l2]\nenabled = true\n[learning.l2]\nenabled = false # never-print-this-secret",
        "[learning.l1]\nmodel = 'never-print-this-secret",
    ] {
        fs::write(home.join("moon.toml"), source).unwrap();
        moon(home)
            .arg("validate")
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("moon.toml is invalid"))
            .stderr(predicate::str::contains("private_marker").not())
            .stderr(predicate::str::contains("never-print-this-secret").not());
        assert!(!home.join("state").exists());
    }
}

#[test]
fn invalid_reasoning_models_and_bounds_are_rejected_without_echoing_values() {
    let temp = tempfile::tempdir().unwrap();
    for (source, field) in [
        (
            "[learning.l1]\nreasoning = 'light'",
            "learning.l1.reasoning",
        ),
        (
            "[learning.l2]\nreasoning = 'never-print-this-secret'",
            "learning.l2.reasoning",
        ),
        (
            "[learning.l1]\nmodel = 'never-print-this-secret'",
            "learning.l1.model",
        ),
        (
            "[learning.l1]\nmodel = 'https://example.com/model'",
            "learning.l1.model",
        ),
        (
            "[learning.l2]\nfallback_model = 'provider/'",
            "learning.l2.fallback_model",
        ),
        ("[learning.l1]\ntimeout_ms = 999", "learning.l1.timeout_ms"),
        (
            "[learning.l2]\ntimeout_ms = 1800001",
            "learning.l2.timeout_ms",
        ),
        (
            "[learning.l1]\nmax_output_tokens = 127",
            "learning.l1.max_output_tokens",
        ),
        (
            "[learning.l2]\nmax_output_tokens = 131073",
            "learning.l2.max_output_tokens",
        ),
        (
            "[learning]\nobservation_ttl_hours = 0",
            "learning.observation_ttl_hours",
        ),
        ("[learning.l2]\nbatch_size = 129", "learning.l2.batch_size"),
        (
            "[learning.l2]\nmax_batches_per_day = 65",
            "learning.l2.max_batches_per_day",
        ),
        ("[learning.l2]\nmax_actions = 33", "learning.l2.max_actions"),
        (
            "[learning.l2]\nmax_input_chars = 1023",
            "learning.l2.max_input_chars",
        ),
        ("[learning.l1]\nprompt_file = ''", "learning.l1.prompt_file"),
        (
            "[learning.l2]\ntimezone = '../never-print-this-secret'",
            "learning.l2.timezone",
        ),
    ] {
        fs::write(temp.path().join("moon.toml"), source).unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains(field))
            .stderr(predicate::str::contains("never-print-this-secret").not());
    }
}

#[test]
fn astra_reasoning_is_validated_for_primary_and_fallback_models() {
    let temp = tempfile::tempdir().unwrap();
    for stage in ["l1", "l2"] {
        for (model_key, reasoning_key) in [
            ("model", "reasoning"),
            ("fallback_model", "fallback_reasoning"),
        ] {
            for reasoning in ["off", "minimal", "adaptive"] {
                fs::write(temp.path().join("moon.toml"), format!(
                    "[learning.{stage}]\n{model_key} = 'openai/gpt-6-astra'\n{reasoning_key} = '{reasoning}'"
                )).unwrap();
                moon(temp.path())
                    .arg("validate")
                    .assert()
                    .failure()
                    .stderr(predicate::str::contains("incompatible with GPT-6-Astra"));
            }
        }
    }
    for reasoning in ["low", "medium", "high", "xhigh", "max", "ultra"] {
        fs::write(
            temp.path().join("moon.toml"),
            format!("[learning.l1]\nmodel = 'openai/gpt-6-astra'\nreasoning = '{reasoning}'"),
        )
        .unwrap();
        moon(temp.path()).arg("validate").assert().success();
    }
}

#[test]
fn credentials_misplaced_in_a_model_field_are_not_printed() {
    let temp = tempfile::tempdir().unwrap();
    for stage in ["l1", "l2"] {
        for field in ["model", "fallback_model"] {
            fs::write(
                temp.path().join("moon.toml"),
                format!("[learning.{stage}]\n{field} = 'openai/sk-proj-placeholder-private-value'"),
            )
            .unwrap();
            moon(temp.path())
                .arg("show")
                .assert()
                .failure()
                .stdout(predicate::str::is_empty())
                .stderr(predicate::str::contains(
                    "authentication belongs to OpenClaw",
                ))
                .stderr(predicate::str::contains("placeholder-private-value").not());
        }
    }
}

#[test]
fn daily_time_requires_exact_valid_twenty_four_hour_format() {
    let temp = tempfile::tempdir().unwrap();
    for value in [
        "", "3:00", "24:00", "00:60", "-1:00", "03.00", "03:00:00", "😀",
    ] {
        fs::write(
            temp.path().join("moon.toml"),
            format!("[learning.l2]\ndaily_at = '{value}'"),
        )
        .unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains("24-hour HH:MM format"));
    }
}

#[test]
fn oversized_and_non_utf8_configuration_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("moon.toml"), vec![b' '; 65_537]).unwrap();
    moon(temp.path())
        .arg("show")
        .assert()
        .failure()
        .stderr(predicate::str::contains("64 KiB"));
    fs::write(temp.path().join("moon.toml"), [0xff, 0xfe, 0xfd]).unwrap();
    moon(temp.path())
        .arg("show")
        .assert()
        .failure()
        .stderr(predicate::str::contains("UTF-8"));
    assert!(!temp.path().join("state").exists());
}

#[test]
fn directory_configuration_is_an_error_not_legacy_defaults() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("moon.toml")).unwrap();
    moon(temp.path())
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("regular file"));
}

#[test]
fn validate_checks_real_timezone_identifiers() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("moon.toml"),
        "[learning.l2]\ntimezone = 'Neverland/Imaginary'",
    )
    .unwrap();
    moon(temp.path()).arg("show").assert().success();
    moon(temp.path())
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a recognised IANA timezone"))
        .stderr(predicate::str::contains("Neverland").not());
    assert!(!temp.path().join("state").exists());
}

#[test]
fn validate_checks_prompt_readability_type_size_and_utf8_without_returning_contents() {
    let temp = tempfile::tempdir().unwrap();
    for stage in ["l1", "l2"] {
        let prompt = temp.path().join("custom.md");
        fs::write(
            temp.path().join("moon.toml"),
            format!("[learning.{stage}]\nprompt_file = 'custom.md'"),
        )
        .unwrap();
        moon(temp.path()).arg("show").assert().success();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains("could not be read"));

        fs::create_dir(&prompt).unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains("regular file"));
        fs::remove_dir(&prompt).unwrap();

        fs::write(&prompt, vec![b'x'; 65_537]).unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains("64 KiB"));

        fs::write(&prompt, [0xff, 0xfe]).unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .failure()
            .stderr(predicate::str::contains("UTF-8"));

        fs::write(&prompt, "Private prompt content must never be printed.").unwrap();
        moon(temp.path())
            .arg("validate")
            .assert()
            .success()
            .stdout(predicate::str::contains("Private prompt content").not());
        fs::remove_file(&prompt).unwrap();
    }
    assert!(!temp.path().join("state").exists());
}

#[cfg(unix)]
#[test]
fn broken_symlink_is_not_treated_as_absent_or_replaced() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("missing-target");
    symlink(&target, temp.path().join("moon.toml")).unwrap();
    moon(temp.path())
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not be read"));
    moon(temp.path())
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing to overwrite"));
    assert_eq!(
        fs::read_link(temp.path().join("moon.toml")).unwrap(),
        target
    );
}

#[cfg(unix)]
#[test]
fn fifo_configuration_does_not_block_validation() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let temp = tempfile::tempdir().unwrap();
    let fifo = CString::new(temp.path().join("moon.toml").as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is NUL-terminated and remains alive for the syscall.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    moon(temp.path())
        .timeout(std::time::Duration::from_secs(5))
        .arg("validate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("regular file"));
}
