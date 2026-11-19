use assert_cmd::Command;
use moon::Store;
use predicates::str::contains;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{fs, path::Path};

const FIRST: &str = "discord:100000000000000011";
const SECOND: &str = "discord:100000000000000022";
const BOT: &str = "bot:discord:100000000000000033";

fn privacy(home: &Path, operation: &str) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("moon"));
    command
        .env_remove("MOON_DATABASE")
        .args(["--dimensions", "64", "--home"])
        .arg(home)
        .args(["--json", "privacy", operation]);
    command
}

fn principals(home: &Path) -> Value {
    let result = privacy(home, "master-discord-principals")
        .assert()
        .success();
    serde_json::from_slice(&result.get_output().stdout).unwrap()
}

fn configure(home: &Path, profile: &str, principal: &str) {
    fs::write(
        home.join("moon.toml"),
        format!("[master]\nprofile_id='{profile}'\nmemory_principal='{principal}'\n"),
    )
    .unwrap();
}

fn bind(home: &Path, sender: &str, owner: &str, n: u32, bot: bool) -> Command {
    let mut command = privacy(home, if bot { "bind-bot" } else { "bind-room" });
    command.args([
        "--sender",
        sender,
        "--admission-key",
        &format!("{n:064x}"),
        "--audience-json",
        &json!([FIRST, SECOND, BOT, sender, owner]).to_string(),
    ]);
    if !bot {
        command.args(["--owners-json", &json!([owner]).to_string()]);
    }
    command
}

fn snapshot<'a>(command: &'a mut Command, profile: &str, principal: &str) -> &'a mut Command {
    command.args([
        "--master-profile-id",
        profile,
        "--master-memory-principal",
        principal,
    ])
}

#[test]
fn raw_discord_protection_is_atomic_and_outlives_config_changes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let db = home.join("state/moon.sqlite");
    configure(home, "person", FIRST);
    bind(home, FIRST, FIRST, 1, false).assert().success();
    assert_eq!(principals(home), json!([]));
    bind(home, FIRST, FIRST, 2, false)
        .args(["--master-profile-id", "person"])
        .assert()
        .failure();
    snapshot(&mut bind(home, BOT, FIRST, 2, true), "former", FIRST)
        .assert()
        .failure()
        .stderr(contains("changed"));
    snapshot(&mut bind(home, FIRST, FIRST, 2, false), "person", SECOND)
        .assert()
        .failure()
        .stderr(contains("changed"));
    snapshot(&mut bind(home, SECOND, SECOND, 2, false), "person", FIRST)
        .assert()
        .failure()
        .stderr(contains("among their owners"));
    snapshot(&mut bind(home, BOT, FIRST, 1, true), "person", FIRST)
        .assert()
        .failure()
        .stderr(contains("conflicting"));
    assert_eq!(principals(home), json!([]));

    // A new restriction and its admission must survive or fail together.
    let connection = Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_anchor BEFORE INSERT ON master_discord_principals
        BEGIN SELECT RAISE(ABORT,'anchor write failed'); END;",
        )
        .unwrap();
    snapshot(&mut bind(home, FIRST, FIRST, 2, false), "person", FIRST)
        .assert()
        .failure()
        .stderr(contains("anchor write failed"));
    let read = Store::open_existing(&db, 64).unwrap();
    assert!(
        read.turn_admission(&format!("{:064x}", 2))
            .unwrap()
            .sender
            .is_none()
    );
    connection
        .execute_batch("DROP TRIGGER reject_anchor; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let checkpointed = fs::read(&db).unwrap();
    let revision = || {
        connection
            .query_row(
                "SELECT revision FROM privacy_revision WHERE id=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let before = revision();
    for _ in 0..2 {
        // The exact existing ordinary admission can later acquire the restriction.
        snapshot(&mut bind(home, FIRST, FIRST, 1, false), "person", FIRST)
            .assert()
            .success();
    }
    // The old admission was reused: only the new privacy restriction advanced
    // the rollback checkpoint. Its row is still solely in SQLite's WAL.
    assert_eq!(revision(), before + 1);
    assert_eq!(fs::read(&db).unwrap(), checkpointed);
    assert_eq!(principals(home), json!([FIRST]));
    configure(home, "person", SECOND);
    bind(home, BOT, SECOND, 3, true).assert().success();
    configure(home, "person", "openclaw:person");
    snapshot(
        &mut bind(home, "openclaw:person", "openclaw:person", 4, false),
        "person",
        "openclaw:person",
    )
    .assert()
    .success();
    fs::remove_file(home.join("moon.toml")).unwrap();
    assert_eq!(principals(home), json!([FIRST, SECOND]));
    assert!(
        connection
            .execute("DELETE FROM master_discord_principals", [])
            .is_err()
    );
    assert!(
        connection
            .execute("UPDATE master_discord_principals SET principal=?1", [FIRST])
            .is_err()
    );
}

#[test]
fn master_principal_probe_does_not_initialise_or_migrate_and_fails_on_incomplete_schema() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("missing-home");
    assert_eq!(principals(&home), json!([]));
    assert!(!home.exists());
    fs::create_dir_all(home.join("state")).unwrap();
    let db = home.join("state/moon.sqlite");
    let connection = Connection::open(&db).unwrap();
    connection.pragma_update(None, "user_version", 19).unwrap();
    drop(connection);
    let before = fs::read(&db).unwrap();
    assert_eq!(principals(&home), json!([]));
    assert_eq!(fs::read(&db).unwrap(), before);
    assert_eq!(fs::read_dir(home.join("state")).unwrap().count(), 1);
    let connection = Connection::open(&db).unwrap();
    connection.pragma_update(None, "user_version", 20).unwrap();
    privacy(&home, "master-discord-principals")
        .assert()
        .failure();
    assert!(
        connection
            .prepare("SELECT 1 FROM master_discord_principals")
            .is_err()
    );
    connection.pragma_update(None, "user_version", 21).unwrap();
    privacy(&home, "master-discord-principals")
        .assert()
        .failure();
}
