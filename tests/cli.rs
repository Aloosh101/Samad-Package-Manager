use assert_cmd::Command;
use predicates::prelude::*;
use std::process::Output;

fn spm() -> Command {
    if let Ok(path) = std::env::var("SPM_BIN") {
        Command::new(path)
    } else {
        Command::cargo_bin("spm").unwrap()
    }
}

/// When spmd is not running, daemon-dependent commands fail with
/// a connection error. We accept that as a valid test outcome
/// so tests don't fail in dev environments without a running daemon.
fn check_or_daemon_off(output: &Output, expected_stderr: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Connection reset")
        || stderr.contains("Connection refused")
        || stderr.contains("Broken pipe")
    {
        return; // daemon not running — acceptable
    }
    assert!(
        stderr.contains(expected_stderr),
        "Expected stderr to contain '{expected_stderr}', got: {stderr}"
    );
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
        .stdout(predicate::str::is_match(r"\d+\.\d+\.\d+").unwrap());
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
    let cmd = spm()
        .args(["remove", "nonexistent-pkg-12345"])
        .assert()
        .failure();
    let output = cmd.get_output();
    check_or_daemon_off(output, "not installed");
}

#[test]
fn test_purge_nonexistent_fails() {
    let cmd = spm()
        .args(["purge", "nonexistent-pkg-12345"])
        .assert()
        .failure();
    let output = cmd.get_output();
    check_or_daemon_off(output, "not installed");
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
    let cmd = spm()
        .arg("upgrade")
        .assert()
        .failure();
    let output = cmd.get_output();
    check_or_daemon_off(output, "Only root can upgrade");
}

#[test]
fn test_repo_list() {
    let cmd = spm()
        .args(["repo", "list"])
        .assert();
    let output = cmd.get_output();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Connection reset")
                || stderr.contains("Connection refused")
                || stderr.contains("Broken pipe"),
            "Unexpected error: {stderr}"
        );
    }
}

#[test]
fn test_config_help() {
    spm()
        .args(["config", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("View or modify"));
}
