use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn update_help_exposes_only_the_approved_first_release_interface() {
    Command::cargo_bin("moon")
        .expect("binary")
        .args(["update", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--check"))
        .stdout(predicate::str::contains("--version"))
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--yes"))
        .stdout(predicate::str::contains("--allow-downgrade"))
        .stdout(predicate::str::contains("--no-restart").not());
}

#[test]
fn invalid_update_mode_is_rejected_before_network_or_storage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_home = temp.path().join("missing runtime");
    Command::cargo_bin("moon")
        .expect("binary")
        .args([
            "--home",
            missing_home.to_str().expect("utf8"),
            "--json",
            "update",
            "--check",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(r#""code":"invalid_arguments""#));
    assert!(!missing_home.exists());
}

#[cfg(unix)]
mod recovery_helper {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    struct Fixture {
        root: tempfile::TempDir,
        home: PathBuf,
        commands: PathBuf,
    }

    impl Fixture {
        fn new(moon_body: &str) -> Self {
            let root = tempfile::tempdir().expect("tempdir");
            let home = root.path().join("Moon runtime with spaces");
            let commands = root.path().join("command paths");
            fs::create_dir_all(home.join("bin")).unwrap();
            fs::create_dir(&commands).unwrap();
            write_executable(
                &home.join("bin/moon"),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > \"$MOON_TEST_ROOT/moon-args\"\nprintf '%s' \"${{PATH%%:*}}\" > \"$MOON_TEST_ROOT/wrapper-dir\"\n{moon_body}\n"
                ),
            );
            write_executable(
                &commands.join("openclaw"),
                "#!/bin/sh\nprintf '<%s>' \"$@\" >> \"$MOON_TEST_ROOT/openclaw-args\"\nprintf '\\n' >> \"$MOON_TEST_ROOT/openclaw-args\"\nexit \"${MOON_TEST_OPENCLAW_EXIT:-0}\"\n",
            );
            write_executable(
                &commands.join("moon"),
                "#!/bin/sh\nprintf '%s\\n' 'shadowed Moon was invoked' >&2\nexit 99\n",
            );
            Self {
                root,
                home,
                commands,
            }
        }

        fn command(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tools/recover-openclaw-update.sh"
                ))
                .current_dir(self.root.path())
                .env("MOON_HOME", &self.home)
                .env("MOON_TEST_ROOT", self.root.path())
                .env("TMPDIR", self.root.path())
                .env("PATH", format!("{}:/usr/bin:/bin", self.commands.display()))
                .env_remove("MOON_TEST_OPENCLAW_EXIT");
            command
        }

        fn read(&self, name: &str) -> String {
            fs::read_to_string(self.root.path().join(name)).unwrap()
        }

        fn assert_cleaned_up(&self) {
            let wrapper_dir = PathBuf::from(self.read("wrapper-dir"));
            assert!(wrapper_dir.starts_with(self.root.path()));
            assert!(!wrapper_dir.exists(), "temporary wrapper was not removed");
        }
    }

    fn write_executable(path: &Path, text: &str) {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn bridge_only_changes_the_exact_legacy_stop_and_uses_canonical_moon() {
        let fixture = Fixture::new(
            r#"openclaw --version || exit
openclaw gateway stop --json || exit
openclaw gateway stop || exit
openclaw gateway stop --json extra || exit
openclaw gateway stop --force --json || exit
openclaw gateway stop --json --force || exit
openclaw gateway start --json || exit
openclaw config validate || exit
openclaw 'argument with spaces' "apostrophe'argument" '$literal' || exit"#,
        );
        fixture
            .command()
            .args(["--version", "2.5.1", "--dry-run"])
            .assert()
            .success();
        assert_eq!(
            fixture.read("moon-args"),
            format!(
                "{}\n--home\n{}\nupdate\n--version\n2.5.1\n--dry-run\n",
                fixture.home.join("bin/moon").display(),
                fixture.home.display()
            )
        );
        assert_eq!(
            fixture.read("openclaw-args"),
            "<--version>\n<gateway><stop><--force><--json>\n<gateway><stop>\n<gateway><stop><--json><extra>\n<gateway><stop><--force><--json>\n<gateway><stop><--json><--force>\n<gateway><start><--json>\n<config><validate>\n<argument with spaces><apostrophe'argument><$literal>\n"
        );
        fixture.assert_cleaned_up();
    }

    #[test]
    fn bridge_preserves_explicit_update_consent_and_cleans_up_after_refusal() {
        let fixture = Fixture::new(
            r#"for argument do
  if [ "$argument" = --yes ]; then
    openclaw gateway stop --json
    exit $?
  fi
done
printf '%s\n' 'authorization_required' >&2
exit 42"#,
        );
        fixture
            .command()
            .assert()
            .code(42)
            .stderr(predicate::str::contains("authorization_required"));
        assert!(!fixture.root.path().join("openclaw-args").exists());
        assert!(!fixture.read("moon-args").contains("--yes"));
        fixture.assert_cleaned_up();

        fixture.command().arg("--yes").assert().success();
        assert!(fixture.read("moon-args").ends_with("update\n--yes\n"));
        assert_eq!(
            fixture.read("openclaw-args"),
            "<gateway><stop><--force><--json>\n"
        );
        fixture.assert_cleaned_up();
    }

    #[test]
    fn bridge_preserves_subprocess_failure_and_cleans_up() {
        let fixture = Fixture::new("openclaw gateway stop --json\nexit $?");
        fixture
            .command()
            .env("MOON_TEST_OPENCLAW_EXIT", "37")
            .arg("--yes")
            .assert()
            .code(37);
        assert_eq!(
            fixture.read("openclaw-args"),
            "<gateway><stop><--force><--json>\n"
        );
        fixture.assert_cleaned_up();
    }

    #[test]
    fn bridge_leaves_interactive_consent_to_the_original_updater() {
        let fixture = Fixture::new(
            r#"printf '%s' 'Apply this Moon update? [y/N] '
IFS= read -r response || exit 41
if [ "$response" != y ]; then
  exit 42
fi
openclaw gateway stop --json
exit $?"#,
        );
        fixture.command().write_stdin("n\n").assert().code(42);
        assert!(!fixture.root.path().join("openclaw-args").exists());
        fixture.assert_cleaned_up();

        fixture
            .command()
            .write_stdin("y\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("Apply this Moon update? [y/N]"));
        assert!(!fixture.read("moon-args").contains("--yes"));
        assert_eq!(
            fixture.read("openclaw-args"),
            "<gateway><stop><--force><--json>\n"
        );
        fixture.assert_cleaned_up();
    }
}
