use semver::{Version, VersionReq};
use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::thread;

use dpp_core::{
    Dependency, PackFormat, WorldManifest, WorldPaths, archive_cache_path, sha512_bytes,
};
use dpp_modrinth::ModrinthClient;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::{solve_world, update_world};

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
