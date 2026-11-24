use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DppError, DppResult};
use crate::manifest::DEFAULT_LOAD_WEIGHT;
use crate::pack_format::PackFormat;
use crate::paths::write_replace_atomic;

pub const LOCK_FILE_VERSION: u32 = 1;

fn default_lock_version() -> u32 {
    LOCK_FILE_VERSION
}

fn default_load_weight() -> u16 {
    DEFAULT_LOAD_WEIGHT
}

/// The world-level `.dpp/lock.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockFile {
    #[serde(default = "default_lock_version")]
    pub lock_version: u32,
    #[serde(default)]
    pub metadata: LockMetadata,
    #[serde(default, rename = "package")]
    pub packages: Vec<LockPackage>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            lock_version: LOCK_FILE_VERSION,
            metadata: LockMetadata::default(),
            packages: Vec::new(),
        }
    }
}

impl LockFile {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let lock = toml::from_str::<Self>(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        lock.validate()?;
        Ok(lock)
    }

    pub fn load_optional(path: &Path) -> DppResult<Option<Self>> {
        match fs::metadata(path) {
            Ok(_) => Self::load(path).map(Some),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(DppError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn validate(&self) -> DppResult<()> {
        if self.lock_version != LOCK_FILE_VERSION {
            return Err(DppError::UnsupportedLockSchema {
                found: self.lock_version,
                supported: LOCK_FILE_VERSION,
            });
        }
        Ok(())
    }

    pub fn target_pack_format(&self) -> Option<PackFormat> {
        self.metadata.pack_format
    }

    pub fn to_toml(&self) -> DppResult<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn write(&self, path: &Path) -> DppResult<()> {
        write_replace_atomic(path, self.to_toml()?.as_bytes())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mc_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_format: Option<PackFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockPackage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    pub version_number: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub sha512: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    pub install_name: String,
    #[serde(default = "default_load_weight")]
    pub load_weight: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
lock_version = 1

[metadata]
mc_version = "26.3"
pack_format = [121, 0]
generated_at = "2026-11-24T12:00:00Z"
resolver = "dpp 0.1.0"

[[package]]
id = "terralith"
source = "modrinth"
project_id = "1a2b3c4d"
version_id = "9x8y7z6w"
version_number = "2.5.3"
sha512 = "abc123"
install_name = "0010_terralith"
load_weight = 10
"#;

    #[test]
    fn parses_lockfile() {
        let lock: LockFile = toml::from_str(EXAMPLE).unwrap();
        lock.validate().unwrap();

        assert_eq!(lock.lock_version, 1);
        assert_eq!(lock.target_pack_format(), Some(PackFormat::new(121, 0)));
        assert_eq!(lock.packages.len(), 1);
        assert_eq!(lock.packages[0].install_name, "0010_terralith");
        assert_eq!(lock.packages[0].load_weight, 10);
    }

    #[test]
    fn serializes_package_as_array_of_tables() {
        let lock: LockFile = toml::from_str(EXAMPLE).unwrap();
        let text = lock.to_toml().unwrap();
        assert!(text.contains("[[package]]"));
        assert_eq!(toml::from_str::<LockFile>(&text).unwrap(), lock);
    }

    #[test]
    fn rejects_future_lock_schema() {
        let lock = LockFile {
            lock_version: LOCK_FILE_VERSION + 1,
            ..LockFile::default()
        };
        assert!(lock.validate().is_err());
    }
}
