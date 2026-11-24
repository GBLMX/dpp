use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn dpp() -> Command {
    Command::cargo_bin("dpp").expect("dpp binary should build")
}

fn world() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("level.dat"), b"test-level").unwrap();
    directory
}

#[test]
fn version_is_available() {
    dpp()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("dpp 0.1.0"));
}

#[test]
fn init_requires_level_dat() {
    let directory = tempfile::tempdir().unwrap();

    dpp()
        .args(["--world", directory.path().to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("level.dat not found"));
}

#[test]
fn init_status_check_round_trip() {
    let directory = world();
    let world = directory.path().to_str().unwrap();

    dpp()
        .args([
            "--world",
            world,
            "init",
            "--mc-version",
            "26.3",
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized dpp world"));

    assert!(directory.path().join("dpp.toml").is_file());
    assert!(directory.path().join(".dpp/staging").is_dir());

    dpp()
        .args(["--world", world, "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Manifest: schema 1"))
        .stdout(predicate::str::contains("Target pack format: 121.0"))
        .stdout(predicate::str::contains("Lock: absent"));

    dpp()
        .args(["--world", world, "check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));

    dpp()
        .args(["--world", world, "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn status_requires_manifest() {
    let directory = world();

    dpp()
        .args(["--world", directory.path().to_str().unwrap(), "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("dpp.toml not found"));
}

#[test]
fn check_rejects_lock_target_mismatch() {
    let directory = world();
    let world = directory.path().to_str().unwrap();

    dpp()
        .args(["--world", world, "init", "--pack-format", "121.0"])
        .assert()
        .success();

    fs::write(
        directory.path().join(".dpp/lock.toml"),
        r#"
lock_version = 1

[metadata]
pack_format = [121, 1]
"#,
    )
    .unwrap();

    dpp()
        .args(["--world", world, "check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not match manifest pack format",
        ));
}
