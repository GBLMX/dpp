use std::fs;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::error::{DppError, DppResult};

/// The Minecraft version information stored in `level.dat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelInfo {
    pub mc_version: String,
    pub snapshot: bool,
    pub data_version: Option<i32>,
}

impl LevelInfo {
    pub fn load(path: &Path) -> DppResult<Self> {
        let bytes = fs::read(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        let decompressed = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(bytes.as_slice());
            let mut output = Vec::new();
            decoder
                .read_to_end(&mut output)
                .map_err(|source| DppError::LevelDatGzip {
                    path: path.to_path_buf(),
                    source,
                })?;
            output
        } else {
            bytes
        };

        let level_dat: LevelDat =
            fastnbt::from_bytes(&decompressed).map_err(|source| DppError::LevelDatNbt {
                path: path.to_path_buf(),
                source,
            })?;

        let version = level_dat
            .data
            .version
            .ok_or_else(|| DppError::LevelDatVersionMissing(path.to_path_buf()))?;
        let mc_version = version
            .name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| DppError::LevelDatVersionMissing(path.to_path_buf()))?;

        Ok(Self {
            mc_version,
            snapshot: version.snapshot,
            data_version: level_dat.data.data_version.or(version.id),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LevelDat {
    data: Data,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Data {
    #[serde(default)]
    version: Option<Version>,
    #[serde(default)]
    data_version: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Version {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    snapshot: bool,
    #[serde(default)]
    id: Option<i32>,
}

#[cfg(test)]
fn write_fixture(path: &Path) {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use serde::Serialize;

    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct FixtureVersion {
        name: String,
        snapshot: bool,
        id: i32,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct FixtureData {
        version: FixtureVersion,
        data_version: i32,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Fixture {
        data: FixtureData,
    }

    let fixture = Fixture {
        data: FixtureData {
            version: FixtureVersion {
                name: "26.3".into(),
                snapshot: false,
                id: 5023,
            },
            data_version: 5023,
        },
    };

    let nbt = fastnbt::to_bytes(&fixture).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt).unwrap();
    fs::write(path, encoder.finish().unwrap()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gzipped_level_dat() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("level.dat");
        write_fixture(&path);

        let info = LevelInfo::load(&path).unwrap();
        assert_eq!(info.mc_version, "26.3");
        assert!(!info.snapshot);
        assert_eq!(info.data_version, Some(5023));
    }

    #[test]
    fn rejects_non_nbt_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("level.dat");
        fs::write(&path, b"not nbt").unwrap();

        assert!(LevelInfo::load(&path).is_err());
    }
}
