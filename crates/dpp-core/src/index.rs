use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DppError, DppResult};
use crate::paths::write_replace_atomic;

pub const INDEX_FILE_VERSION: u32 = 1;

fn default_index_version() -> u32 {
    INDEX_FILE_VERSION
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFile {
    #[serde(default = "default_index_version")]
    pub index_version: u32,
    #[serde(default, rename = "package")]
    pub packages: Vec<IndexPackage>,
}

impl Default for IndexFile {
    fn default() -> Self {
        Self {
            index_version: INDEX_FILE_VERSION,
            packages: Vec::new(),
        }
    }
}

impl IndexFile {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let index = toml::from_str::<Self>(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        index.validate()?;
        Ok(index)
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
        if self.index_version != INDEX_FILE_VERSION {
            return Err(DppError::UnsupportedIndexSchema {
                found: self.index_version,
                supported: INDEX_FILE_VERSION,
            });
        }
        Ok(())
    }

    pub fn write(&self, path: &Path) -> DppResult<()> {
        let text = toml::to_string(self)?;
        write_replace_atomic(path, text.as_bytes())
    }

    pub fn find(&self, id: &str, source: &str) -> Option<&IndexPackage> {
        self.packages
            .iter()
            .find(|package| package.id == id && package.source == source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexPackage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    pub sha512: String,
    pub install_name: String,
    #[serde(default, rename = "file")]
    pub files: Vec<IndexFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFileEntry {
    pub path: String,
    pub sha512: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
index_version = 1

[[package]]
id = "terralith"
source = "modrinth"
version_id = "abc"
sha512 = "deadbeef"
install_name = "0050_terralith"

[[package.file]]
path = "pack.mcmeta"
sha512 = "cafe"
"#;

    #[test]
    fn parses_index() {
        let index: IndexFile = toml::from_str(EXAMPLE).unwrap();
        index.validate().unwrap();
        assert_eq!(index.packages.len(), 1);
        assert_eq!(index.packages[0].files[0].path, "pack.mcmeta");
    }

    #[test]
    fn round_trips_index() {
        let index: IndexFile = toml::from_str(EXAMPLE).unwrap();
        let text = toml::to_string(&index).unwrap();
        assert!(text.contains("[[package]]"));
        assert!(text.contains("[[package.file]]"));
        assert_eq!(toml::from_str::<IndexFile>(&text).unwrap(), index);
    }

    #[test]
    fn rejects_future_schema() {
        let index = IndexFile {
            index_version: INDEX_FILE_VERSION + 1,
            ..IndexFile::default()
        };
        assert!(index.validate().is_err());
    }
}
