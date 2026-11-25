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

#[test]
fn inspect_prints_pack_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("pack.zip");
    let file = fs::File::create(&archive).unwrap();
    let mut writer = ZipWriter::new(file);
    for (name, contents) in [
        (
            "pack.mcmeta",
            r#"{"pack":{"description":"test","min_format":[121,0],"max_format":[121,0]}}"#,
        ),
        (
            "dpp.toml",
            r#"
[package]
id = "smithed:stdlib"
version = "0.3.1"

[dependencies]
"smithed:events" = ">=0.2.0, <0.4.0"
"#,
        ),
    ] {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(contents.as_bytes()).unwrap();
    }
    writer.finish().unwrap();

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pack: smithed:stdlib 0.3.1"))
        .stdout(predicate::str::contains("dependencies: smithed:events"));
}

#[test]
fn host_requires_server_root() {
    dpp()
        .args(["host", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--server-root"));
}

#[test]
fn host_check_detects_and_writes_lock() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("paper-26.3.jar"), b"jar").unwrap();
    fs::create_dir(directory.path().join("plugins")).unwrap();
    fs::write(
        directory.path().join("plugins/dpp-adapter-paper.jar"),
        b"jar",
    )
    .unwrap();
    fs::create_dir_all(directory.path().join(".dpp")).unwrap();
    fs::write(
        directory.path().join(".dpp/host-provided.toml"),
        r#"
[[pack]]
id = "DppAdapterPaper/bootstrap"
plugin = "DppAdapterPaper"
version = "0.1.0"
enabled = true
"#,
    )
    .unwrap();

    dpp()
        .args([
            "--server-root",
            directory.path().to_str().unwrap(),
            "host",
            "check",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("kind: paper"))
        .stdout(predicate::str::contains("dpp-adapter-paper.jar"))
        .stdout(predicate::str::contains("DppAdapterPaper/bootstrap"));

    let lock = fs::read_to_string(directory.path().join(".dpp/host.lock")).unwrap();
    assert!(lock.contains("kind = \"paper\""));
    assert!(lock.contains("dpp-adapter-paper.jar"));
    assert!(lock.contains("DppAdapterPaper/bootstrap"));
    assert!(lock.contains("[[provided_pack]]"));

    dpp()
        .args([
            "--server-root",
            directory.path().to_str().unwrap(),
            "host",
            "status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Observed host:"))
        .stdout(predicate::str::contains("host.lock:"));
}

#[test]
fn snapshot_save_list_restore() {
    let directory = world();
    let world = directory.path().to_str().unwrap();
    dpp().args(["--world", world, "init"]).assert().success();

    dpp()
        .args(["--world", world, "snapshot", "save", "first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("saved snapshot first"));

    dpp()
        .args(["--world", world, "snapshot", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("first"));

    dpp()
        .args(["--world", world, "snapshot", "restore", "first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("restored snapshot first"));
}

#[test]
fn config_allow_apply_and_status() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/app.toml"), "a = 1\n").unwrap();
    let patch = root.join("patch.toml");
    fs::write(
        &patch,
        r#"
target = "config/app.toml"
format = "toml"
merge = "merge"

[data]
b = 2
"#,
    )
    .unwrap();
    let root_arg = root.to_str().unwrap();

    dpp()
        .args([
            "--server-root",
            root_arg,
            "config",
            "allow",
            "config/app.toml",
        ])
        .assert()
        .success();
    dpp()
        .args([
            "--server-root",
            root_arg,
            "config",
            "apply",
            patch.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("applied config patch"));
    dpp()
        .args(["--server-root", root_arg, "config", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config/app.toml"));
    let value = fs::read_to_string(root.join("config/app.toml")).unwrap();
    assert!(value.contains("a = 1"));
    assert!(value.contains("b = 2"));
}

#[test]
fn adapter_manifest_and_event_validation() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("dpp-adapter.toml");
    fs::write(
        &manifest,
        r#"
[adapter]
id = "dpp-adapter-paper"
version = "0.1.0"
protocol = "dpp.events/1"
capabilities = ["dpp.events/1"]
events = ["dpp:boss_phase"]
"#,
    )
    .unwrap();
    let event = directory.path().join("event.json");
    fs::write(
        &event,
        r#"{"protocol":"dpp.events/1","id":1,"event":"dpp:boss_phase","payload":{"phase":2}}"#,
    )
    .unwrap();

    dpp()
        .args(["adapter", "validate", manifest.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("dpp-adapter-paper"));
    dpp()
        .args([
            "adapter",
            "event",
            manifest.to_str().unwrap(),
            event.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok: event dpp:boss_phase"));

    fs::write(
        &event,
        r#"{"protocol":"dpp.events/1","id":2,"event":"dpp:not_declared"}"#,
    )
    .unwrap();
    dpp()
        .args([
            "adapter",
            "event",
            manifest.to_str().unwrap(),
            event.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not declared"));
}

#[test]
fn status_reports_host_provided_collision() {
    let world = world();
    let world_arg = world.path().to_str().unwrap();
    dpp()
        .args(["--world", world_arg, "init"])
        .assert()
        .success();
    fs::write(
        world.path().join(".dpp/index.toml"),
        r#"
index_version = 1
[[package]]
id = "demo"
source = "modrinth"
sha512 = "abc"
install_name = "DppAdapterPaper/bootstrap"
[[package.file]]
path = "pack.mcmeta"
sha512 = "def"
"#,
    )
    .unwrap();

    let server = tempfile::tempdir().unwrap();
    fs::create_dir_all(server.path().join(".dpp")).unwrap();
    fs::write(
        server.path().join(".dpp/host-provided.toml"),
        r#"
[[pack]]
id = "DppAdapterPaper/bootstrap"
plugin = "DppAdapterPaper"
"#,
    )
    .unwrap();

    dpp()
        .args([
            "--world",
            world_arg,
            "--server-root",
            server.path().to_str().unwrap(),
            "status",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "collides with a dpp-managed package",
        ));
}

#[test]
fn new_creates_beet_compatible_scaffold() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("demo");
    dpp()
        .args(["new", "demo", "--dir", project.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("created datapack project"));

    assert!(project.join("dpp.toml").is_file());
    assert!(project.join("beet.json").is_file());
    assert!(project.join("src/pack.mcmeta").is_file());
    assert!(
        project
            .join("src/data/demo/function/load.mcfunction")
            .is_file()
    );
    let manifest = fs::read_to_string(project.join("dpp.toml")).unwrap();
    assert!(manifest.contains("id = \"local:demo\""));
    assert!(manifest.contains("pack_format = [121, 0]"));

    let archive = directory.path().join("demo.zip");
    write_tree_zip(&project.join("src"), &archive);
    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success();

    dpp()
        .args(["new", "demo", "--dir", project.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

fn write_tree_zip(root: &Path, zip_path: &Path) {
    fn add_tree(writer: &mut ZipWriter<std::fs::File>, root: &Path, current: &Path) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                add_tree(writer, root, &path);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                writer
                    .start_file(
                        name,
                        SimpleFileOptions::default()
                            .compression_method(CompressionMethod::Deflated),
                    )
                    .unwrap();
                writer.write_all(&fs::read(&path).unwrap()).unwrap();
            }
        }
    }

    let file = fs::File::create(zip_path).unwrap();
    let mut writer = ZipWriter::new(file);
    add_tree(&mut writer, root, root);
    writer.finish().unwrap();
}

#[test]
fn stdlib_inspects_as_valid_pack() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("stdlib.zip");
    write_tree_zip(&root, &archive);

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pack: dpp:stdlib 0.1.0"));
}

#[test]
fn demo_inspects_as_valid_pack() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo");
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("demo.zip");
    write_tree_zip(&root, &archive);

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pack: local:demo 0.1.0"))
        .stdout(predicate::str::contains("dependencies: dpp:stdlib"));
}

#[test]
fn new_rejects_minecraft_namespace() {
    let directory = tempfile::tempdir().unwrap();
    dpp()
        .args([
            "new",
            "minecraft",
            "--dir",
            directory.path().join("pack").to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved"));
    assert!(!directory.path().join("pack").exists());
}

#[test]
fn build_pack_builds_scaffold_project() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("demo");
    let archive = directory.path().join("demo.zip");
    dpp()
        .args(["new", "demo", "--dir", project.to_str().unwrap()])
        .assert()
        .success();

    dpp()
        .args([
            "build-pack",
            project.to_str().unwrap(),
            "--output",
            archive.to_str().unwrap(),
            "--url",
            "https://example.invalid/demo.zip",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"local:demo\""))
        .stdout(predicate::str::contains(
            "\"url\": \"https://example.invalid/demo.zip\"",
        ));

    dpp()
        .args([
            "inspect",
            archive.to_str().unwrap(),
            "--pack-format",
            "121.0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pack: local:demo 0.1.0"));
}

#[test]
fn update_writes_lock_for_empty_world() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "update"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already up to date"));
    assert!(world.path().join(".dpp/lock.toml").is_file());
}

#[test]
fn apply_records_next_start_and_supports_dry_run() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "update"])
        .assert()
        .success();

    let applied = world.path().join(".dpp/applied.toml");
    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "apply",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("would apply"));
    assert!(!applied.exists());

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "apply"])
        .assert()
        .success();
    let text = fs::read_to_string(&applied).unwrap();
    assert!(text.contains("method = \"next_start\""));
    assert!(text.contains("verification = \"unverified\""));
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Applied:"))
        .stdout(predicate::str::contains("via next_start"));

    fs::remove_file(&applied).unwrap();
    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "apply",
            "--manual",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restart or rejoin"));
    assert!(
        fs::read_to_string(&applied)
            .unwrap()
            .contains("method = \"manual\"")
    );
}

#[test]
fn apply_rcon_sends_reload() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (id, kind, body) = read_rcon_packet(&mut stream);
        assert_eq!((id, kind, body.as_slice()), (1, 3, b"secret".as_slice()));
        write_rcon_packet(&mut stream, 1, 2, b"");
        let (id, kind, body) = read_rcon_packet(&mut stream);
        assert_eq!((id, kind, body.as_slice()), (2, 2, b"reload".as_slice()));
        write_rcon_packet(&mut stream, 2, 0, b"reloaded");
        let (id, kind, body) = read_rcon_packet(&mut stream);
        assert_eq!(
            (id, kind, body.as_slice()),
            (2, 2, b"datapack list enabled".as_slice())
        );
        write_rcon_packet(&mut stream, 2, 0, b"There are 0 data packs enabled");
    });

    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "update"])
        .assert()
        .success();

    let server_root = tempfile::tempdir().unwrap();
    fs::write(
        server_root.path().join("server.properties"),
        format!("enable-rcon=true\nrcon.port={port}\n"),
    )
    .unwrap();
    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "--server-root",
            server_root.path().to_str().unwrap(),
            "apply",
            "--rcon",
        ])
        .env("DPP_RCON_PASSWORD", "secret")
        .assert()
        .success()
        .stdout(predicate::str::contains("reload sent through RCON"));

    server.join().unwrap();
    let applied = fs::read_to_string(world.path().join(".dpp/applied.toml")).unwrap();
    assert!(applied.contains("method = \"rcon\""));
    assert!(applied.contains("verification = \"pack_list_ok\""));
}

fn read_rcon_packet(stream: &mut std::net::TcpStream) -> (i32, i32, Vec<u8>) {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut packet = vec![0u8; i32::from_le_bytes(length) as usize];
    stream.read_exact(&mut packet).unwrap();
    let id = i32::from_le_bytes(packet[0..4].try_into().unwrap());
    let kind = i32::from_le_bytes(packet[4..8].try_into().unwrap());
    (id, kind, packet[8..packet.len() - 2].to_vec())
}

fn write_rcon_packet(stream: &mut std::net::TcpStream, id: i32, kind: i32, body: &[u8]) {
    let length = (body.len() + 10) as i32;
    stream.write_all(&length.to_le_bytes()).unwrap();
    stream.write_all(&id.to_le_bytes()).unwrap();
    stream.write_all(&kind.to_le_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.write_all(&[0, 0]).unwrap();
}

#[test]
fn install_stage_writes_plan_and_apply_aborts() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "install",
            "--stage",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("staged plan:"));
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Staged: txn-"));
    assert!(!world.path().join(".dpp/lock.toml").exists());

    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "apply",
            "--abort",
        ])
        .assert()
        .success();
    let mut plans = Vec::new();
    for entry in fs::read_dir(world.path().join(".dpp/staging")).unwrap() {
        let path = entry.unwrap().path().join("plan.toml");
        if path.is_file() {
            plans.push(path);
        }
    }
    assert!(plans.is_empty());
}

#[test]
fn status_reports_pending_journal_without_recovering_it() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();
    fs::write(
        world.path().join(".dpp/journal.toml"),
        r#"journal_version = 1
transaction = "tx"
phase = "committing"
staging = ".dpp/staging/tx"
backup_names = []
existing_names = []
installed_names = []
removed_names = []
"#,
    )
    .unwrap();

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Journal: committing transaction tx",
        ));
    assert!(world.path().join(".dpp/journal.toml").is_file());
}

fn write_order_lock(world: &Path) {
    fs::create_dir_all(world.join(".dpp")).unwrap();
    fs::write(
        world.join(".dpp/lock.toml"),
        r#"lock_version = 1

[[package]]
id = "a"
source = "modrinth"
version_number = "1.0.0"
sha512 = "a"
install_name = "0010_a"
load_weight = 10

[[package]]
id = "b"
source = "modrinth"
version_number = "1.0.0"
sha512 = "b"
install_name = "0020_b"
load_weight = 20
"#,
    )
    .unwrap();
}

#[test]
fn order_reports_drift_and_commands() {
    let directory = tempfile::tempdir().unwrap();
    let enabled = ["vanilla", "file/0020_b", "file/0010_a"];
    write_level_dat_packs(directory.path(), "26.3", false, Some((&enabled, &[])));
    write_order_lock(directory.path());

    dpp()
        .args([
            "--world",
            directory.path().to_str().unwrap(),
            "order",
            "--print-commands",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Expected: 0010_a, 0020_b"))
        .stdout(predicate::str::contains("Observed: 0020_b, 0010_a"))
        .stdout(predicate::str::contains("Order: drift"))
        .stdout(predicate::str::contains(
            "/datapack enable \"file/0010_a\" before \"file/0020_b\"",
        ));

    dpp()
        .args([
            "--world",
            directory.path().to_str().unwrap(),
            "order",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"drift\""));
}

#[test]
fn order_reports_unknown_when_world_may_be_running() {
    let directory = tempfile::tempdir().unwrap();
    write_level_dat_packs(
        directory.path(),
        "26.3",
        false,
        Some((&["file/0010_a"], &[])),
    );
    write_order_lock(directory.path());
    fs::write(directory.path().join("session.lock"), b"").unwrap();

    dpp()
        .args(["--world", directory.path().to_str().unwrap(), "order"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Observed: unknown"))
        .stdout(predicate::str::contains("Order: unknown"));
}

#[test]
fn order_without_lock_reports_empty_expected() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "order"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Expected: none"));
}

#[test]
fn remove_cleans_orphaned_packages() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    fs::write(
        world.path().join("dpp.toml"),
        "schema = 1\n\n[dependencies]\nroot = \"*\"\n",
    )
    .unwrap();
    let hash = "a".repeat(128);
    fs::write(
        world.path().join(".dpp/lock.toml"),
        format!(
            r#"lock_version = 1

[[package]]
id = "root"
source = "modrinth"
version_number = "1.0.0"
sha512 = "{hash}"
install_name = "0010_root"
load_weight = 10

[[package]]
id = "child"
source = "modrinth"
version_number = "1.0.0"
sha512 = "{hash}"
install_name = "0020_child"
load_weight = 20

[[package]]
id = "orphan"
source = "modrinth"
version_number = "1.0.0"
sha512 = "{hash}"
install_name = "0030_orphan"
load_weight = 30
"#
        ),
    )
    .unwrap();

    for (install_name, package_id, dependencies) in [
        ("0010_root", "root", "\n[dependencies]\nchild = \"*\"\n"),
        ("0020_child", "child", ""),
        ("0030_orphan", "orphan", ""),
    ] {
        let directory = world.path().join("datapacks").join(install_name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("dpp.toml"),
            format!("[package]\nid = \"{package_id}\"\nversion = \"1.0.0\"\n{dependencies}"),
        )
        .unwrap();
    }

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "remove", "root"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "removed orphaned packages: 0020_child, 0030_orphan",
        ));

    for install_name in ["0010_root", "0020_child", "0030_orphan"] {
        assert!(!world.path().join("datapacks").join(install_name).exists());
    }
}

#[test]
fn check_detects_modified_managed_file() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    let content = b"original";
    let file_hash = dpp_core::sha512_bytes(content);
    let archive_hash = "a".repeat(128);
    fs::write(
        world.path().join(".dpp/lock.toml"),
        format!(
            r#"lock_version = 1

[[package]]
id = "demo"
source = "modrinth"
version_number = "1.0.0"
sha512 = "{archive_hash}"
install_name = "demo"
load_weight = 50
"#
        ),
    )
    .unwrap();
    fs::write(
        world.path().join(".dpp/index.toml"),
        format!(
            r#"index_version = 1

[[package]]
id = "demo"
source = "modrinth"
sha512 = "{archive_hash}"
install_name = "demo"

[[package.file]]
path = "pack.mcmeta"
sha512 = "{file_hash}"
"#
        ),
    )
    .unwrap();
    let package = world.path().join("datapacks/demo");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("pack.mcmeta"), content).unwrap();

    dpp()
        .args(["--world", world.path().to_str().unwrap(), "check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok:"));

    fs::write(package.join("pack.mcmeta"), b"modified").unwrap();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("sha512 mismatch"));
}

#[test]
fn install_locked_and_offline_require_lockfile() {
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();

    for flag in ["--locked", "--offline"] {
        dpp()
            .args(["--world", world.path().to_str().unwrap(), "install", flag])
            .assert()
            .failure()
            .stderr(predicate::str::contains("requires an existing lock.toml"));
    }
}

#[test]
fn add_offline_requires_metadata_cache() {
    let cache = tempfile::tempdir().unwrap();
    let world = world();
    dpp()
        .args(["--world", world.path().to_str().unwrap(), "init"])
        .assert()
        .success();
    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "update",
            "--offline",
        ])
        .env("XDG_CACHE_HOME", cache.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("already up to date"));
    dpp()
        .args([
            "--world",
            world.path().to_str().unwrap(),
            "add",
            "demo",
            "--offline",
        ])
        .env("XDG_CACHE_HOME", cache.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("metadata cache is missing"));
}
