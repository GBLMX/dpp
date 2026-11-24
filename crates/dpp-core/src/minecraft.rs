use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::{DppError, DppResult};
use crate::pack_format::PackFormat;

/// Release versions whose data pack format is known without reading a jar.
///
/// Keep this table in sync with `docs/research-datapack.md`. Snapshots are
/// resolved from the installed client/server `version.json` instead.
const RELEASE_FORMATS: &[(&str, u32, u32)] = &[
    ("26.3", 121, 0),
    ("26.2", 107, 1),
    ("26.1.2", 101, 1),
    ("26.1.1", 101, 1),
    ("26.1", 101, 1),
    ("1.21.11", 94, 1),
    ("1.21.10", 88, 0),
    ("1.21.9", 88, 0),
    ("1.21.8", 81, 0),
    ("1.21.7", 81, 0),
    ("1.21.6", 80, 0),
    ("1.21.5", 71, 0),
    ("1.21.4", 61, 0),
    ("1.21.3", 57, 0),
    ("1.21.2", 57, 0),
    ("1.21.1", 48, 0),
    ("1.21", 48, 0),
    ("1.20.6", 41, 0),
    ("1.20.5", 41, 0),
    ("1.20.4", 26, 0),
    ("1.20.3", 26, 0),
    ("1.20.2", 18, 0),
    ("1.20.1", 15, 0),
    ("1.20", 15, 0),
    ("1.19.4", 12, 0),
    ("1.19.3", 10, 0),
    ("1.19.2", 10, 0),
    ("1.19.1", 10, 0),
    ("1.19", 10, 0),
    ("1.18.2", 9, 0),
    ("1.18.1", 8, 0),
    ("1.18", 8, 0),
    ("1.17.1", 7, 0),
    ("1.17", 7, 0),
    ("1.16.5", 6, 0),
    ("1.16.4", 6, 0),
    ("1.16.3", 6, 0),
    ("1.16.2", 6, 0),
    ("1.16.1", 5, 0),
    ("1.16", 5, 0),
    ("1.15.2", 5, 0),
    ("1.15.1", 5, 0),
    ("1.15", 5, 0),
    ("1.14.4", 4, 0),
    ("1.14.3", 4, 0),
    ("1.14.2", 4, 0),
    ("1.14.1", 4, 0),
    ("1.14", 4, 0),
    ("1.13.2", 4, 0),
    ("1.13.1", 4, 0),
    ("1.13", 4, 0),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JarVersion {
    pub id: Option<String>,
    pub pack_format: Option<PackFormat>,
}

/// Read `version.json` from a client/server jar.
///
/// Returns `None` when the jar has no `version.json` entry.
pub fn read_version_json_from_jar(path: &Path) -> DppResult<Option<JarVersion>> {
    let file = File::open(path).map_err(|source| DppError::JarOpen {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| DppError::JarZip {
        path: path.to_path_buf(),
        source,
    })?;

    let mut entry = match archive.by_name("version.json") {
        Ok(entry) => entry,
        Err(ZipError::FileNotFound) => return Ok(None),
        Err(source) => {
            return Err(DppError::JarZip {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|source| DppError::JarEntry {
            path: path.to_path_buf(),
            source,
        })?;

    let json: VersionJson = serde_json::from_str(&text).map_err(|source| DppError::JarJson {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(Some(JarVersion {
        id: json.id,
        pack_format: json.pack_version.as_ref().and_then(parse_pack_version),
    }))
}

#[derive(Debug, Deserialize)]
struct VersionJson {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    pack_version: Option<Value>,
}

fn json_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn format_from_pair(major: Option<&Value>, minor: Option<&Value>) -> Option<PackFormat> {
    Some(PackFormat::new(json_u32(major)?, json_u32(minor)?))
}

fn parse_pack_version(value: &Value) -> Option<PackFormat> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(PackFormat::from),
        Value::Object(map) => {
            if let Some(format) = format_from_pair(map.get("data_major"), map.get("data_minor")) {
                return Some(format);
            }

            if let Some(data) = map.get("data") {
                return parse_pack_version(data);
            }

            format_from_pair(map.get("major"), map.get("minor"))
        }
        _ => None,
    }
}

/// The default Minecraft directory, honoring `MINECRAFT_PATH`, then platform defaults.
pub fn minecraft_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MINECRAFT_PATH") {
        return Some(PathBuf::from(path));
    }

    #[cfg(target_os = "windows")]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata).join(".minecraft"));
    }

    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".minecraft"))
}

fn find_version_jar_in(minecraft_dir: &Path, mc_version: &str) -> Option<PathBuf> {
    let directory = minecraft_dir.join("versions").join(mc_version);
    let jar = directory.join(format!("{mc_version}.jar"));
    jar.is_file().then_some(jar)
}

/// The client jar for a version, if the standard Minecraft directory has it.
pub fn find_version_jar(mc_version: &str) -> Option<PathBuf> {
    find_version_jar_in(&minecraft_dir()?, mc_version)
}

/// A `server.jar` next to the world directory, e.g. `server/world` -> `server/server.jar`.
pub fn adjacent_server_jar(world: &Path) -> Option<PathBuf> {
    world
        .parent()
        .map(|parent| parent.join("server.jar"))
        .filter(|path| path.is_file())
}

/// Resolve a data pack format from a jar, then from the built-in release table.
pub fn detect_pack_format(
    mc_version: &str,
    snapshot: bool,
    jar: Option<&Path>,
) -> DppResult<PackFormat> {
    if let Some(jar) = jar {
        if let Some(version) = read_version_json_from_jar(jar)? {
            if let Some(found) = version.id.as_deref() {
                if found != mc_version {
                    return Err(DppError::JarVersionMismatch {
                        path: jar.to_path_buf(),
                        expected: mc_version.to_owned(),
                        found: found.to_owned(),
                    });
                }
            }
            if let Some(format) = version.pack_format {
                return Ok(format);
            }
        }
    }

    pack_format_for_release(mc_version).ok_or_else(|| DppError::PackFormatUnknown {
        mc_version: mc_version.to_owned(),
        snapshot,
    })
}

pub fn pack_format_for_release(mc_version: &str) -> Option<PackFormat> {
    RELEASE_FORMATS
        .iter()
        .find(|(version, _, _)| *version == mc_version)
        .map(|(_, major, minor)| PackFormat::new(*major, *minor))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn write_jar(path: &Path, version_json: &str) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "version.json",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(version_json.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn maps_release_versions() {
        assert_eq!(
            pack_format_for_release("26.3"),
            Some(PackFormat::new(121, 0))
        );
        assert_eq!(
            pack_format_for_release("1.20.5"),
            Some(PackFormat::new(41, 0))
        );
        assert_eq!(
            pack_format_for_release("1.21.9"),
            Some(PackFormat::new(88, 0))
        );
        assert_eq!(
            pack_format_for_release("1.21.11"),
            Some(PackFormat::new(94, 1))
        );
        assert_eq!(pack_format_for_release("1.12.2"), None);
    }

    #[test]
    fn reads_current_version_json_shape() {
        let directory = tempfile::tempdir().unwrap();
        let jar = directory.path().join("26.3.jar");
        write_jar(
            &jar,
            r#"{"id":"26.3","pack_version":{"resource_major":97,"resource_minor":1,"data_major":121,"data_minor":0}}"#,
        );

        let version = read_version_json_from_jar(&jar).unwrap().unwrap();
        assert_eq!(version.id.as_deref(), Some("26.3"));
        assert_eq!(version.pack_format, Some(PackFormat::new(121, 0)));
        assert_eq!(
            detect_pack_format("26.3", false, Some(&jar)).unwrap(),
            PackFormat::new(121, 0)
        );
    }

    #[test]
    fn reads_legacy_data_shape() {
        let directory = tempfile::tempdir().unwrap();
        let jar = directory.path().join("1.20.5.jar");
        write_jar(
            &jar,
            r#"{"id":"1.20.5","pack_version":{"resource":32,"data":41}}"#,
        );

        assert_eq!(
            read_version_json_from_jar(&jar)
                .unwrap()
                .unwrap()
                .pack_format,
            Some(PackFormat::new(41, 0))
        );
    }

    #[test]
    fn falls_back_to_release_table_without_jar() {
        assert_eq!(
            detect_pack_format("26.3", false, None).unwrap(),
            PackFormat::new(121, 0)
        );
    }

    #[test]
    fn rejects_snapshot_without_jar() {
        assert!(detect_pack_format("26.4-pre-1", true, None).is_err());
    }

    #[test]
    fn rejects_mismatched_jar_version() {
        let directory = tempfile::tempdir().unwrap();
        let jar = directory.path().join("server.jar");
        write_jar(&jar, r#"{"id":"1.20.4","pack_version":{"data":26}}"#);

        assert!(detect_pack_format("1.20.5", false, Some(&jar)).is_err());
    }

    #[test]
    fn locates_client_and_adjacent_server_jars() {
        let root = tempfile::tempdir().unwrap();
        let version_dir = root.path().join("versions/26.3");
        std::fs::create_dir_all(&version_dir).unwrap();
        let client_jar = version_dir.join("26.3.jar");
        std::fs::write(&client_jar, b"jar").unwrap();
        assert_eq!(find_version_jar_in(root.path(), "26.3"), Some(client_jar));

        let server = tempfile::tempdir().unwrap();
        let world = server.path().join("world");
        std::fs::create_dir(&world).unwrap();
        let server_jar = server.path().join("server.jar");
        std::fs::write(&server_jar, b"jar").unwrap();
        assert_eq!(adjacent_server_jar(&world), Some(server_jar));
    }
}
