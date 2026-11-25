use semver::{Version, VersionReq};
use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::thread;

use dpp_core::{
    DEFAULT_LOAD_WEIGHT, Dependency, IndexFile, IndexPackage, LockFile, LockPackage, PackFormat,
    WorldManifest, WorldPaths, archive_cache_path, sha512_bytes,
};
use dpp_modrinth::ModrinthClient;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::{add_package, solve_world, update_world};
use crate::solver::{preferences_from_lock, solve_world_with_preferences};

fn pack_zip(manifest: &str) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    for (name, contents) in [
        (
            "pack.mcmeta",
            r#"{"pack":{"description":"test","min_format":[121,0],"max_format":[121,0]}}"#,
        ),
        ("dpp.toml", manifest),
    ] {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(contents.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn legacy_pack_zip() -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    writer
        .start_file(
            "pack.mcmeta",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
    writer
        .write_all(
            br#"{"pack":{"description":"legacy","min_format":[121,0],"max_format":[121,0]}}"#,
        )
        .unwrap();
    writer.finish().unwrap().into_inner()
}

fn version_json(
    project_id: &str,
    version_id: &str,
    version_number: &str,
    file_url: &str,
    sha512: &str,
    size: usize,
) -> String {
    format!(
        r#"[{{"id":"{version_id}","project_id":"{project_id}","name":"test","version_number":"{version_number}","dependencies":[],"game_versions":["26.3"],"loaders":["datapack"],"version_type":"release","files":[{{"filename":"pack.zip","hashes":{{"sha512":"{sha512}"}},"url":"{file_url}","primary":true,"size":{size}}}]}}]"#
    )
}

#[test]
fn resolves_transitive_dependency_from_pack_manifest() {
    let root_zip = pack_zip(
        r#"
[package]
id = "test:root"
version = "1.0.0"

[dependencies]
"test:dep" = { version = ">=1.0.0, <2.0.0", project = "dep-project" }
"#,
    );
    let dep_zip = pack_zip(
        r#"
[package]
id = "test:dep"
version = "1.2.0"
"#,
    );
    let root_sha = sha512_bytes(&root_zip);
    let dep_sha = sha512_bytes(&dep_zip);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");

    let root_json = version_json(
        "root-project",
        "root-version",
        "1.0.0",
        &format!("{base_url}/root.zip"),
        &root_sha,
        root_zip.len(),
    );
    let dep_json = version_json(
        "dep-project",
        "dep-version",
        "1.2.0",
        &format!("{base_url}/dep.zip"),
        &dep_sha,
        dep_zip.len(),
    );
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/root-project/version") {
                    ("200 OK", "application/json", root_json.as_bytes().to_vec())
                } else if path.starts_with("/v2/project/dep-project/version") {
                    ("200 OK", "application/json", dep_json.as_bytes().to_vec())
                } else if path == "/root.zip" {
                    ("200 OK", "application/zip", root_zip.clone())
                } else if path == "/dep.zip" {
                    ("200 OK", "application/zip", dep_zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let mut manifest = WorldManifest::default();
    manifest.dependencies.insert(
        "root-project".to_owned(),
        Dependency::Version("*".to_owned()),
    );
    let solved = solve_world(&manifest, "26.3", PackFormat::new(121, 0), &client).unwrap();
    server.join().unwrap();

    let sources: BTreeSet<_> = solved
        .iter()
        .map(|package| package.source.as_str())
        .collect();
    assert_eq!(sources, BTreeSet::from(["dep-project", "root-project"]));
    let dep = solved
        .iter()
        .find(|package| package.source == "dep-project")
        .unwrap();
    assert_eq!(dep.manifest.as_ref().unwrap().package.id, "test:dep");

    let _ = std::fs::remove_file(archive_cache_path(&root_sha));
    let _ = std::fs::remove_file(archive_cache_path(&dep_sha));
}

#[test]
fn wildcard_matches_legacy_synthetic_versions() {
    let legacy = Version::parse("0.0.0+legacy.0").unwrap();
    assert!(VersionReq::parse("*").unwrap().matches(&legacy));
    assert!(!VersionReq::parse(">=1.0.0").unwrap().matches(&legacy));
}

#[test]
fn resolves_legacy_non_semver_version() {
    let zip = legacy_pack_zip();
    let sha = sha512_bytes(&zip);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let json = version_json(
        "legacy-project",
        "legacy-version",
        "v1.0",
        &format!("{base_url}/legacy.zip"),
        &sha,
        zip.len(),
    );
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/legacy-project/version") {
                    ("200 OK", "application/json", json.as_bytes().to_vec())
                } else if path == "/legacy.zip" {
                    ("200 OK", "application/zip", zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let mut manifest = WorldManifest::default();
    manifest.dependencies.insert(
        "legacy-project".to_owned(),
        Dependency::Version("*".to_owned()),
    );
    let solved = solve_world(&manifest, "26.3", PackFormat::new(121, 0), &client).unwrap();
    server.join().unwrap();
    assert_eq!(solved.len(), 1);
    assert_eq!(solved[0].version.version_number, "v1.0");
    let _ = std::fs::remove_file(archive_cache_path(&sha));
}

#[test]
fn update_installs_resolved_packages() {
    let root_zip = pack_zip(
        r#"
[package]
id = "test:update"
version = "1.0.0"
"#,
    );
    let sha = sha512_bytes(&root_zip);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let json = version_json(
        "update-project",
        "update-version",
        "1.0.0",
        &format!("{base_url}/update.zip"),
        &sha,
        root_zip.len(),
    );
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/update-project/version") {
                    ("200 OK", "application/json", json.as_bytes().to_vec())
                } else if path == "/update.zip" {
                    ("200 OK", "application/zip", root_zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let world = tempfile::tempdir().unwrap();
    std::fs::write(world.path().join("level.dat"), b"fixture").unwrap();
    let paths = WorldPaths::discover(Some(world.path())).unwrap();
    paths.ensure_state_dirs().unwrap();
    let mut manifest = WorldManifest::default();
    manifest.dependencies.insert(
        "update-project".to_owned(),
        Dependency::Version("*".to_owned()),
    );
    manifest.write(&paths.manifest()).unwrap();

    let report = update_world(
        &paths,
        "26.3",
        PackFormat::new(121, 0),
        &client,
        None,
        false,
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(report.installed.len(), 1);
    assert!(paths.lock().is_file());
    assert!(paths.index().is_file());
    assert!(paths.root().join("datapacks/0050_update-project").is_dir());
    let _ = std::fs::remove_file(archive_cache_path(&sha));
}

#[test]
fn prefers_locked_version_when_solving_add() {
    let root_zip = pack_zip(
        r#"
[package]
id = "test:root"
version = "1.0.0"

[dependencies]
"test:dep" = { version = ">=1.0.0, <2.0.0", project = "dep-project" }
"#,
    );
    let old_zip = pack_zip(
        r#"
[package]
id = "test:dep"
version = "1.0.0"
"#,
    );
    let old_size = old_zip.len();
    let new_zip = pack_zip(
        r#"
[package]
id = "test:dep"
version = "1.1.0"
"#,
    );
    let root_sha = sha512_bytes(&root_zip);
    let old_sha = sha512_bytes(&old_zip);
    let new_sha = sha512_bytes(&new_zip);
    for sha in [&root_sha, &old_sha, &new_sha] {
        let _ = std::fs::remove_file(archive_cache_path(sha));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let root_json = version_json(
        "root-project",
        "root-version",
        "1.0.0",
        &format!("{base_url}/root.zip"),
        &root_sha,
        root_zip.len(),
    );
    let dep_json = format!(
        "[{},{}]",
        version_json(
            "dep-project",
            "dep-new",
            "1.1.0",
            &format!("{base_url}/dep-new.zip"),
            &new_sha,
            new_zip.len(),
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
        version_json(
            "dep-project",
            "dep-old",
            "1.0.0",
            &format!("{base_url}/dep-old.zip"),
            &old_sha,
            old_zip.len(),
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
    );
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/root-project/version") {
                    ("200 OK", "application/json", root_json.as_bytes().to_vec())
                } else if path.starts_with("/v2/project/dep-project/version") {
                    ("200 OK", "application/json", dep_json.as_bytes().to_vec())
                } else if path == "/root.zip" {
                    ("200 OK", "application/zip", root_zip.clone())
                } else if path == "/dep-old.zip" {
                    ("200 OK", "application/zip", old_zip.clone())
                } else if path == "/dep-new.zip" {
                    ("200 OK", "application/zip", new_zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let mut manifest = WorldManifest::default();
    manifest.dependencies.insert(
        "root-project".to_owned(),
        Dependency::Version("*".to_owned()),
    );
    let lock = LockFile {
        packages: vec![LockPackage {
            id: "dep-project".to_owned(),
            package_id: Some("test:dep".to_owned()),
            source: "modrinth".to_owned(),
            project_id: Some("dep-project".to_owned()),
            version_id: Some("dep-old".to_owned()),
            version_number: "1.0.0".to_owned(),
            published_at: None,
            sha512: old_sha.clone(),
            sha1: None,
            size: Some(old_size as u64),
            url: None,
            file_name: None,
            manifest_hash: None,
            install_name: "0050_dep".to_owned(),
            load_weight: DEFAULT_LOAD_WEIGHT,
        }],
        ..LockFile::default()
    };
    let preferences = preferences_from_lock(&lock);
    let solved = solve_world_with_preferences(
        &manifest,
        "26.3",
        PackFormat::new(121, 0),
        &client,
        &preferences,
    )
    .unwrap();
    server.join().unwrap();

    let dep = solved
        .iter()
        .find(|package| package.source == "dep-project")
        .unwrap();
    assert_eq!(dep.version.id, "dep-old");
    let _ = std::fs::remove_file(archive_cache_path(&root_sha));
    let _ = std::fs::remove_file(archive_cache_path(&old_sha));
    let _ = std::fs::remove_file(archive_cache_path(&new_sha));
}

#[test]
fn add_with_existing_lock_keeps_locked_dependency() {
    let root_zip = pack_zip(
        r#"
[package]
id = "test:root"
version = "1.0.0"
# add-with-existing-lock root

[dependencies]
"test:dep" = { version = ">=1.0.0, <2.0.0", project = "dep-project" }
"#,
    );
    let old_zip = pack_zip(
        r#"
[package]
id = "test:dep"
version = "1.0.0"
# add-with-existing-lock old
"#,
    );
    let new_zip = pack_zip(
        r#"
[package]
id = "test:dep"
version = "1.1.0"
# add-with-existing-lock new
"#,
    );
    let old_size = old_zip.len();
    let root_sha = sha512_bytes(&root_zip);
    let old_sha = sha512_bytes(&old_zip);
    let new_sha = sha512_bytes(&new_zip);
    for sha in [&root_sha, &old_sha, &new_sha] {
        let _ = std::fs::remove_file(archive_cache_path(sha));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let root_json = version_json(
        "root-project",
        "root-version",
        "1.0.0",
        &format!("{base_url}/root.zip"),
        &root_sha,
        root_zip.len(),
    );
    let dep_json = format!(
        "[{},{}]",
        version_json(
            "dep-project",
            "dep-new",
            "1.1.0",
            &format!("{base_url}/dep-new.zip"),
            &new_sha,
            new_zip.len(),
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
        version_json(
            "dep-project",
            "dep-old",
            "1.0.0",
            &format!("{base_url}/dep-old.zip"),
            &old_sha,
            old_size,
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
    );
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/root-project/version") {
                    ("200 OK", "application/json", root_json.as_bytes().to_vec())
                } else if path.starts_with("/v2/project/dep-project/version") {
                    ("200 OK", "application/json", dep_json.as_bytes().to_vec())
                } else if path == "/root.zip" {
                    ("200 OK", "application/zip", root_zip.clone())
                } else if path == "/dep-old.zip" {
                    ("200 OK", "application/zip", old_zip.clone())
                } else if path == "/dep-new.zip" {
                    ("200 OK", "application/zip", new_zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
    let paths = WorldPaths::discover(Some(directory.path())).unwrap();
    paths.ensure_state_dirs().unwrap();
    WorldManifest::default()
        .write_new(&paths.manifest())
        .unwrap();
    let dep_lock = LockPackage {
        id: "dep-project".to_owned(),
        package_id: Some("test:dep".to_owned()),
        source: "modrinth".to_owned(),
        project_id: Some("dep-project".to_owned()),
        version_id: Some("dep-old".to_owned()),
        version_number: "1.0.0".to_owned(),
        published_at: None,
        sha512: old_sha.clone(),
        sha1: None,
        size: Some(old_size as u64),
        url: None,
        file_name: None,
        manifest_hash: None,
        install_name: "0050_dep".to_owned(),
        load_weight: 10,
    };
    LockFile {
        packages: vec![dep_lock.clone()],
        ..LockFile::default()
    }
    .write(&paths.lock())
    .unwrap();
    IndexFile {
        packages: vec![IndexPackage {
            id: "dep-project".to_owned(),
            package_id: Some("test:dep".to_owned()),
            source: "modrinth".to_owned(),
            version_id: Some("dep-old".to_owned()),
            sha512: old_sha.clone(),
            install_name: "0050_dep".to_owned(),
            files: Vec::new(),
        }],
        ..IndexFile::default()
    }
    .write(&paths.index())
    .unwrap();
    std::fs::create_dir_all(paths.root().join("datapacks/0050_dep")).unwrap();

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let report = add_package(
        &paths,
        "root-project",
        "26.3",
        PackFormat::new(121, 0),
        &client,
        None,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(report.package.id, "root-project");
    let lock = LockFile::load(&paths.lock()).unwrap();
    let dep = lock
        .packages
        .iter()
        .find(|package| package.id == "dep-project")
        .unwrap();
    assert_eq!(dep.version_id.as_deref(), Some("dep-old"));
    assert_eq!(dep.install_name, "0050_dep");
    assert_eq!(dep.load_weight, 10);
    assert!(
        lock.packages
            .iter()
            .any(|package| package.id == "root-project")
    );
    assert!(
        paths
            .root()
            .join("datapacks")
            .join(&report.package.install_name)
            .is_dir()
    );
    assert_eq!(IndexFile::load(&paths.index()).unwrap().packages.len(), 2);
    let _ = std::fs::remove_file(archive_cache_path(&root_sha));
    let _ = std::fs::remove_file(archive_cache_path(&old_sha));
    let _ = std::fs::remove_file(archive_cache_path(&new_sha));
}

#[test]
fn backtracking_conflict_selects_compatible_old_version() {
    let a2_zip = pack_zip(
        r#"
[package]
id = "test:a"
version = "2.0.0"

[conflicts]
"test:b" = "*"
"#,
    );
    let a1_zip = pack_zip(
        r#"
[package]
id = "test:a"
version = "1.0.0"
"#,
    );
    let b_zip = pack_zip(
        r#"
[package]
id = "test:b"
version = "1.0.0"
"#,
    );
    let a2_sha = sha512_bytes(&a2_zip);
    let a1_sha = sha512_bytes(&a1_zip);
    let b_sha = sha512_bytes(&b_zip);
    for sha in [&a2_sha, &a1_sha, &b_sha] {
        let _ = std::fs::remove_file(archive_cache_path(sha));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let base_url = format!("http://{address}");
    let a_versions = format!(
        "[{},{}]",
        version_json(
            "a-project",
            "a-v2",
            "2.0.0",
            &format!("{base_url}/a2.zip"),
            &a2_sha,
            a2_zip.len(),
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
        version_json(
            "a-project",
            "a-v1",
            "1.0.0",
            &format!("{base_url}/a1.zip"),
            &a1_sha,
            a1_zip.len(),
        )
        .trim_start_matches('[')
        .trim_end_matches(']'),
    );
    let b_versions = version_json(
        "b-project",
        "b-v1",
        "1.0.0",
        &format!("{base_url}/b.zip"),
        &b_sha,
        b_zip.len(),
    );
    let server = thread::spawn(move || {
        for _ in 0..7 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default();
            let (status, content_type, body): (&str, &str, Vec<u8>) =
                if path.starts_with("/v2/project/a-project/version") {
                    ("200 OK", "application/json", a_versions.as_bytes().to_vec())
                } else if path.starts_with("/v2/project/b-project/version") {
                    ("200 OK", "application/json", b_versions.as_bytes().to_vec())
                } else if path == "/a2.zip" {
                    ("200 OK", "application/zip", a2_zip.clone())
                } else if path == "/a1.zip" {
                    ("200 OK", "application/zip", a1_zip.clone())
                } else if path == "/b.zip" {
                    ("200 OK", "application/zip", b_zip.clone())
                } else {
                    ("404 Not Found", "text/plain", b"not found".to_vec())
                };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });

    let client =
        ModrinthClient::with_base_url(format!("{base_url}/v2"), "dpp-solver-test").unwrap();
    let mut manifest = WorldManifest::default();
    for project in ["a-project", "b-project"] {
        manifest
            .dependencies
            .insert(project.to_owned(), Dependency::Version("*".to_owned()));
    }
    let solved = solve_world(&manifest, "26.3", PackFormat::new(121, 0), &client).unwrap();
    server.join().unwrap();

    let a = solved
        .iter()
        .find(|package| package.source == "a-project")
        .unwrap();
    assert_eq!(a.version.id, "a-v1");
    assert_eq!(a.version.version_number, "1.0.0");
    let _ = std::fs::remove_file(archive_cache_path(&a2_sha));
    let _ = std::fs::remove_file(archive_cache_path(&a1_sha));
    let _ = std::fs::remove_file(archive_cache_path(&b_sha));
}
