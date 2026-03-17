use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_lists_core_commands() {
    Command::cargo_bin("lazytmux")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("restore"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("migrate"));
}
