use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::{DppError, DppResult};
use crate::manifest::Dependency;
use crate::pack_format::PackFormat;

pub const PACK_MANIFEST_SCHEMA: u32 = 1;
const PACK_MANIFEST_FILE: &str = "dpp.toml";
const MAX_PACK_MANIFEST_BYTES: u64 = 1024 * 1024;

fn default_schema() -> u32 {
    PACK_MANIFEST_SCHEMA
}

/// Optional `dpp.toml` at the root of a data pack zip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackManifest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub package: PackageSection,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    pub conflicts: BTreeMap<String, String>,
    #[serde(default)]
    pub host: HostSection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSection {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mc_versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_format: Option<PackFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_format: Option<PackFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_format: Option<PackFormat>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbids: Vec<String>,
}

impl PackManifest {
    pub fn from_toml_str(text: &str, path: &Path) -> DppResult<Self> {
        let manifest = toml::from_str::<Self>(text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Read `dpp.toml` from the zip root. A missing file means a legacy pack.
    pub fn load_from_zip(path: &Path) -> DppResult<Option<Self>> {
        let file = File::open(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut archive = ZipArchive::new(file).map_err(|source| DppError::PackZip {
            path: path.to_path_buf(),
            source,
        })?;

        let text = match archive.by_name(PACK_MANIFEST_FILE) {
            Ok(entry) => {
                if entry.is_dir() {
                    return Err(DppError::PackManifestInvalid {
                        path: path.to_path_buf(),
                        reason: "dpp.toml is a directory".to_owned(),
                    });
                }
                if entry.size() > MAX_PACK_MANIFEST_BYTES {
                    return Err(DppError::PackManifestTooLarge {
                        path: path.to_path_buf(),
                        size: entry.size(),
                    });
                }

                let mut text = String::new();
                entry
                    .take(MAX_PACK_MANIFEST_BYTES + 1)
                    .read_to_string(&mut text)
                    .map_err(|source| DppError::Read {
                        path: path.to_path_buf(),
                        source,
                    })?;
                if text.len() as u64 > MAX_PACK_MANIFEST_BYTES {
                    return Err(DppError::PackManifestTooLarge {
                        path: path.to_path_buf(),
                        size: text.len() as u64,
                    });
                }
                text
            }
            Err(ZipError::FileNotFound) => return Ok(None),
            Err(source) => {
                return Err(DppError::PackZip {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        Self::from_toml_str(&text, path).map(Some)
    }

    pub fn validate(&self, path: &Path) -> DppResult<()> {
        if self.schema != PACK_MANIFEST_SCHEMA {
            return Err(DppError::UnsupportedPackManifestSchema {
                found: self.schema,
                supported: PACK_MANIFEST_SCHEMA,
            });
        }
        validate_package_id(&self.package.id, path)?;
        if semver::Version::parse(&self.package.version).is_err() {
            return Err(DppError::PackManifestInvalid {
                path: path.to_path_buf(),
                reason: format!(
                    "package.version {:?} is not valid SemVer",
                    self.package.version
                ),
            });
        }
        match (self.package.min_format, self.package.max_format) {
            (Some(min), Some(max)) if min <= max => {}
            (Some(_), Some(_)) | (Some(_), None) | (None, Some(_)) => {
                return Err(DppError::PackManifestInvalid {
                    path: path.to_path_buf(),
                    reason:
                        "package.min_format/package.max_format must both be present and ordered"
                            .to_owned(),
                });
            }
            (None, None) => {}
        }
        if let (Some(format), Some(min), Some(max)) = (
            self.package.pack_format,
            self.package.min_format,
            self.package.max_format,
        ) {
            if format < min || format > max {
                return Err(DppError::PackManifestInvalid {
                    path: path.to_path_buf(),
                    reason: format!("package.pack_format {format} is outside [{min}, {max}]"),
                });
            }
        }

        for id in self.dependencies.keys().chain(self.conflicts.keys()) {
            validate_package_id(id, path)?;
        }
        for token in self.host.requires.iter().chain(self.host.forbids.iter()) {
            if token.is_empty()
                || token.len() > 128
                || !token.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "./_-:".contains(character)
                })
            {
                return Err(DppError::PackManifestInvalid {
                    path: path.to_path_buf(),
                    reason: format!("invalid host capability token {token:?}"),
                });
            }
        }
        Ok(())
    }
}

fn validate_package_id(id: &str, path: &Path) -> DppResult<()> {
    let mut segments = id.split(':');
    let namespace = segments.next().unwrap_or_default();
    let name = segments.next();
    if namespace.is_empty()
        || id.len() > 128
        || segments.next().is_some()
        || name.is_some_and(|name| name.is_empty())
        || !namespace.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || "_-.".contains(character)
        })
        || name.is_some_and(|name| {
            !name.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || "_-.".contains(character)
            })
        })
    {
        return Err(DppError::PackManifestInvalid {
            path: path.to_path_buf(),
            reason: format!("invalid canonical package id {id:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    const VALID: &str = r#"
schema = 1

[package]
id = "smithed:stdlib"
version = "0.3.1"
mc_versions = ["26.3"]
pack_format = [121, 0]

[dependencies]
"smithed:events" = ">=0.2.0, <0.4.0"

[conflicts]
"legacy:battle" = "*"

[host]
requires = ["dpp.events/1", "paper"]
"#;

    #[test]
    fn parses_pack_manifest() {
        let manifest = PackManifest::from_toml_str(VALID, Path::new("demo.zip")).unwrap();
        assert_eq!(manifest.package.id, "smithed:stdlib");
        assert_eq!(manifest.package.version, "0.3.1");
        assert_eq!(manifest.dependencies.len(), 1);
        assert_eq!(manifest.conflicts.len(), 1);
        assert_eq!(manifest.host.requires, vec!["dpp.events/1", "paper"]);
    }

    #[test]
    fn rejects_non_semver_or_bad_id() {
        let bad_version = VALID.replace("version = \"0.3.1\"", "version = \"latest\"");
        assert!(PackManifest::from_toml_str(&bad_version, Path::new("demo.zip")).is_err());

        let bad_id = VALID.replace("id = \"smithed:stdlib\"", "id = \"Smithed:stdlib\"");
        assert!(PackManifest::from_toml_str(&bad_id, Path::new("demo.zip")).is_err());
    }

    #[test]
    fn accepts_digits_in_package_ids() {
        let manifest = VALID.replace("id = \"smithed:stdlib\"", "id = \"smithed2:event2\"");
        let parsed = PackManifest::from_toml_str(&manifest, Path::new("demo.zip")).unwrap();
        assert_eq!(parsed.package.id, "smithed2:event2");
    }

    #[test]
    fn reads_manifest_from_zip() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "pack.mcmeta",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(b"{}").unwrap();
        writer
            .start_file(
                "dpp.toml",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(VALID.as_bytes()).unwrap();
        writer.finish().unwrap();

        let manifest = PackManifest::load_from_zip(&archive).unwrap().unwrap();
        assert_eq!(manifest.package.id, "smithed:stdlib");
    }

    #[test]
    fn missing_manifest_is_legacy() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "pack.mcmeta",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap();
        assert!(PackManifest::load_from_zip(&archive).unwrap().is_none());
    }
}
