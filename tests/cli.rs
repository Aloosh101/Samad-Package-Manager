use assert_cmd::Command;
use predicates::prelude::*;

fn spm() -> Command {
    if let Ok(path) = std::env::var("SPM_BIN") {
        Command::new(path)
    } else {
        Command::cargo_bin("spm").unwrap()
    }
}

#[test]
fn test_help() {
    spm()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Samad Package Manager"));
}

#[test]
fn test_version() {
    spm()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.1.0"));
}

#[test]
fn test_install_no_args_fails() {
    spm()
        .arg("install")
        .assert()
        .failure();
}

#[test]
fn test_remove_nonexistent_fails() {
    spm()
        .args(["remove", "nonexistent-pkg-12345"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not installed"));
}

#[test]
fn test_purge_nonexistent_fails() {
    spm()
        .args(["purge", "nonexistent-pkg-12345"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not installed"));
}

#[test]
fn test_search_help() {
    spm()
        .args(["search", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Search"));
}

#[test]
fn test_cleanup_help() {
    spm()
        .args(["cleanup", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cleanup"));
}

#[test]
fn test_upgrade_no_args() {
    spm()
        .arg("upgrade")
        .assert()
        .success();
}

#[test]
fn test_repo_list() {
    spm()
        .args(["repo", "list"])
        .assert()
        .success();
}

#[test]
fn test_config_help() {
    spm()
        .args(["config", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("View or modify"));
}
