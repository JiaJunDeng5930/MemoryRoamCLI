use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn command(temp_dir: &TempDir) -> Command {
    let mut command = Command::cargo_bin("memoryroam").expect("binary should build");
    command
        .arg("--db")
        .arg(temp_dir.path().join("notes.sqlite3"));
    command
}

#[test]
fn cli_round_trip_supports_init_create_read_and_aliases() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));

    command(&temp_dir)
        .args(["create", "--content", "Topic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("- 1"));

    command(&temp_dir)
        .args(["alias", "add", "--id", "1", "--text", "topic"])
        .assert()
        .success();

    command(&temp_dir)
        .args(["create", "--content", "See {{topic}}"])
        .assert()
        .success()
        .stdout(predicate::str::contains("- 2"));

    command(&temp_dir)
        .args(["read", "--id", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Incoming links:"))
        .stdout(predicate::str::contains("2 -> See {{1::topic}}"));

    command(&temp_dir)
        .args(["alias", "list", "--id", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("topic"));
}

#[test]
fn cli_delete_blocks_referenced_nodes() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();
    command(&temp_dir)
        .args(["create", "--content", "Topic"])
        .assert()
        .success();
    command(&temp_dir)
        .args(["create", "--content", "Ref {{1}}"])
        .assert()
        .success();

    command(&temp_dir)
        .args(["delete", "--id", "1", "--cascade"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still references node 1"));
}

#[test]
fn cli_update_accepts_single_line_stdin_with_trailing_newline() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();
    command(&temp_dir)
        .args(["create", "--content", "Topic"])
        .assert()
        .success();

    command(&temp_dir)
        .args(["update", "--id", "1"])
        .write_stdin("Updated from stdin\n")
        .assert()
        .success();

    command(&temp_dir)
        .args(["read", "--id", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Content: Updated from stdin"));
}

#[test]
fn cli_read_does_not_create_missing_database_files() {
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let database_path = temp_dir.path().join("notes.sqlite3");

    command(&temp_dir)
        .args(["read", "--id", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unable to open database file"));

    assert!(!database_path.exists());
}

#[test]
fn cli_multiline_create_rolls_back_on_later_failure() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .args(["create", "--content", "First\nRef {{Missing}}"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "lookup `Missing` did not match any node",
        ));

    command(&temp_dir)
        .args(["list", "--top-level"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}
