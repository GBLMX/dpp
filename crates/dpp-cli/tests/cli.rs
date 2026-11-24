use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;

use assert_cmd::Command;
use flate2::Compression;
use flate2::write::GzEncoder;
use predicates::prelude::*;
use serde::Serialize;
use tempfile::TempDir;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

fn dpp() -> Command {
    Command::cargo_bin("dpp").expect("dpp binary should build")
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FixtureVersion {
    name: String,
    snapshot: bool,
    id: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FixtureDataPacks {
    enabled: Vec<String>,
    disabled: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FixtureData {
    version: FixtureVersion,
    data_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_packs: Option<FixtureDataPacks>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Fixture {
    data: FixtureData,
}

fn write_level_dat(world: &Path, version: &str, snapshot: bool) {
    write_level_dat_packs(world, version, snapshot, None);
}

fn write_level_dat_packs(
    world: &Path,
    version: &str,
    snapshot: bool,
    packs: Option<(&[&str], &[&str])>,
) {
    let data_version = if snapshot { 5100 } else { 5023 };
    let data_packs = packs.map(|(enabled, disabled)| FixtureDataPacks {
        enabled: enabled.iter().map(ToString::to_string).collect(),
        disabled: disabled.iter().map(ToString::to_string).collect(),
    });
    let fixture = Fixture {
        data: FixtureData {
            version: FixtureVersion {
                name: version.to_owned(),
                snapshot,
                id: data_version,
            },
            data_version,
            data_packs,
        },
    };

    let nbt = fastnbt::to_bytes(&fixture).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt).unwrap();
    fs::write(world.join("level.dat"), encoder.finish().unwrap()).unwrap();
}

fn write_pack_zip(path: &Path, pack_meta: &str) {
    let file = fs::File::create(path).unwrap();
    let mut writer = ZipWriter::new(file);
    writer
        .start_file(
            "pack.mcmeta",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(pack_meta.as_bytes()).unwrap();
    writer.finish().unwrap();
}

fn world_with_version(version: &str, snapshot: bool) -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    write_level_dat(directory.path(), version, snapshot);
    directory
}

fn world() -> TempDir {
    world_with_version("26.3", false)
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
fn init_detects_version_and_pack_format() {
    let directory = world();
    let world = directory.path().to_str().unwrap();

    dpp()
        .args(["--world", world, "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("detected Minecraft 26.3"))
        .stdout(predicate::str::contains("using data pack format 121.0"));

    let manifest = fs::read_to_string(directory.path().join("dpp.toml")).unwrap();
    assert!(manifest.contains("mc_version = \"26.3\""));
    assert!(manifest.contains("pack_format = [121, 0]"));

    dpp()
        .args(["--world", world, "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Detected pack format: 121.0"));
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

#[test]
fn check_rejects_world_version_mismatch() {
    let directory = world();
    let world = directory.path().to_str().unwrap();

    dpp().args(["--world", world, "init"]).assert().success();

    let manifest_path = directory.path().join("dpp.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("26.3", "26.2")
        .replace("[121, 0]", "[107, 1]");
    fs::write(manifest_path, manifest).unwrap();

    dpp()
        .args(["--world", world, "check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "manifest targets Minecraft 26.2, but world is 26.3",
        ));
}

#[test]
fn check_rejects_world_pack_format_mismatch() {
    let directory = world();
    let world = directory.path().to_str().unwrap();

    dpp().args(["--world", world, "init"]).assert().success();

    let manifest_path = directory.path().join("dpp.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("[121, 0]", "[107, 1]");
    fs::write(manifest_path, manifest).unwrap();

    dpp()
        .args(["--world", world, "check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "manifest targets pack format 107.1, but world is 121.0",
        ));
}

#[test]
fn snapshot_without_jar_requires_explicit_format() {
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    fs::create_dir(&world).unwrap();
    write_level_dat(&world, "26.4-pre-1", true);

    let empty_minecraft = tempfile::tempdir().unwrap();

    dpp()
        .env("MINECRAFT_PATH", empty_minecraft.path())
        .args(["--world", world.to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not determine data pack format",
        ));
}

#[test]
fn search_rejects_invalid_limit() {
    dpp()
        .args(["search", "incendium", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--limit must be between 1 and 100",
        ));
}

#[test]
fn inspect_accepts_and_rejects_pack_zip() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("pack.zip");
    write_pack_zip(
        &archive,
        r#"{
            "pack": {
                "description": "test",
                "min_format": [121, 0],
                "max_format": [121, 0]
            }
        }"#,
    );

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"))
        .stdout(predicate::str::contains("pack format: 121.0"));

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "88.0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not support data pack format 88.0",
        ));
}
