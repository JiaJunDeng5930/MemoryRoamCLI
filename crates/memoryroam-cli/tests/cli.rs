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

fn output_text(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).expect("stdout should be utf8")
}

fn parse_bracketed_id(output: &str) -> String {
    let start = output.find('[').expect("output should contain '['") + 1;
    let end = output[start..]
        .find(']')
        .map(|offset| start + offset)
        .expect("output should contain ']'");
    output[start..end].to_owned()
}

#[test]
fn cli_note_and_day_show_today_entries() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir)
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));

    let first_output = output_text(
        command(&temp_dir)
            .args(["note", "Check issue 7 scope"])
            .assert()
            .success(),
    );
    let second_output = output_text(
        command(&temp_dir)
            .args(["note", "Design the interaction flow"])
            .assert()
            .success(),
    );

    let first_id = parse_bracketed_id(&first_output);
    let second_id = parse_bracketed_id(&second_output);

    command(&temp_dir)
        .arg("day")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "-[{first_id}] Check issue 7 scope"
        )))
        .stdout(predicate::str::contains(format!(
            "-[{second_id}] Design the interaction flow"
        )));
}

#[test]
fn cli_empty_today_hint_preserves_selected_database() {
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let database_path = temp_dir.path().join("notes.sqlite3");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .arg("day")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "database path: {}",
            database_path.display()
        )))
        .stdout(predicate::str::contains(
            "use: memoryroam --db <database-path> note \"...\"",
        ));
}

#[test]
fn cli_empty_today_hint_quotes_database_paths_with_spaces() {
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let database_path = temp_dir.path().join("notes with spaces.sqlite3");

    let mut init = Command::cargo_bin("memoryroam").expect("binary should build");
    init.arg("--db").arg(&database_path).arg("init");
    init.assert().success();

    let mut day = Command::cargo_bin("memoryroam").expect("binary should build");
    day.arg("--db").arg(&database_path).arg("day");
    day.assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "database path: {}",
            database_path.display()
        )))
        .stdout(predicate::str::contains(
            "use: memoryroam --db <database-path> note \"...\"",
        ));
}

#[test]
fn cli_root_create_and_apply_rewrite_selected_nodes() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    let first_output = output_text(
        command(&temp_dir)
            .args(["note", "Software engineering is an engineering discipline"])
            .assert()
            .success(),
    );
    let second_output = output_text(
        command(&temp_dir)
            .args(["note", "Software engineering started in xx year"])
            .assert()
            .success(),
    );

    let first_id = parse_bracketed_id(&first_output);
    let second_id = parse_bracketed_id(&second_output);

    let root_output = output_text(
        command(&temp_dir)
            .args(["root", "create", "Software engineering"])
            .assert()
            .success(),
    );
    let root_id = parse_bracketed_id(&root_output);

    command(&temp_dir)
        .args([
            "root", "apply", &root_id, "--node", &first_id, "--node", &second_id,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "-[{first_id}] {{{{{root_id}::Software engineering}}}} is an engineering discipline"
        )))
        .stdout(predicate::str::contains(format!(
            "-[{second_id}] {{{{{root_id}::Software engineering}}}} started in xx year"
        )));
}

#[test]
fn cli_read_uses_structural_markers_instead_of_field_labels() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    let first_output = output_text(
        command(&temp_dir)
            .args(["note", "Previous sibling"])
            .assert()
            .success(),
    );
    let current_output = output_text(
        command(&temp_dir)
            .args(["note", "Current node"])
            .assert()
            .success(),
    );
    let next_output = output_text(
        command(&temp_dir)
            .args(["note", "Next sibling"])
            .assert()
            .success(),
    );

    let first_id = parse_bracketed_id(&first_output);
    let current_id = parse_bracketed_id(&current_output);
    let next_id = parse_bracketed_id(&next_output);

    command(&temp_dir)
        .args(["read", &current_id])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "-[{first_id}] Previous sibling"
        )))
        .stdout(predicate::str::contains(format!(
            "=[{current_id}] Current node"
        )))
        .stdout(predicate::str::contains(format!(
            "-[{next_id}] Next sibling"
        )))
        .stdout(predicate::str::contains("Content:").not());
}

#[test]
fn cli_failed_note_does_not_leave_an_empty_daily_note() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .args(["note", "See {{Missing}}"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "lookup `Missing` did not match any node",
        ));

    command(&temp_dir)
        .arg("day")
        .assert()
        .success()
        .stdout(predicate::str::contains("empty"));
}

#[test]
fn cli_day_rejects_invalid_dates() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .args(["day", "2026-99-99"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid day date"));
}

#[test]
fn cli_empty_historical_day_does_not_suggest_note_command() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .args(["day", "2026-04-11"])
        .assert()
        .success()
        .stdout(predicate::str::contains("empty"))
        .stdout(predicate::str::contains("use: memoryroam note").not());
}

#[test]
fn cli_accepts_text_arguments_that_start_with_a_dash() {
    let temp_dir = TempDir::new().expect("temp dir should exist");

    command(&temp_dir).arg("init").assert().success();

    command(&temp_dir)
        .args(["note", "- first item"])
        .assert()
        .success()
        .stdout(predicate::str::contains("- first item"));

    command(&temp_dir)
        .args(["root", "create", "- topic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("root ["));
}
