use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn cli_help_lists_core_commands() {
    Command::cargo_bin("tmup")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--tpm").not())
        .stdout(predicate::str::contains("--config-mode").not())
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("restore"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("list"));
}

#[test]
fn cli_help_hides_internal_config_mode_switches() {
    Command::cargo_bin("tmup")
        .unwrap()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--tpm").not())
        .stdout(predicate::str::contains("--config-mode").not())
        .stdout(predicate::str::contains("--bootstrap").not())
        .stdout(predicate::str::contains("--ui-child").not());
}
