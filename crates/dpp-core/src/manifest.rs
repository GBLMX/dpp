use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DppError, DppResult};
use crate::pack_format::PackFormat;
use crate::paths::write_new_atomic;

pub const MANIFEST_SCHEMA: u32 = 1;
pub const DEFAULT_LOAD_WEIGHT: u16 = 50;

fn default_schema() -> u32 {
    MANIFEST_SCHEMA
}

fn default_load_weight() -> u16 {
    DEFAULT_LOAD_WEIGHT
}

/// The world-level `dpp.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldManifest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default)]
    pub world: WorldSection,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    #[serde(default)]
    pub order: OrderSection,
}

impl Default for WorldManifest {
    fn default() -> Self {
        Self {
            schema: MANIFEST_SCHEMA,
            world: WorldSection::default(),
            dependencies: BTreeMap::new(),
            order: OrderSection::default(),
        }
    }
}

impl WorldManifest {
    pub fn new(mc_version: Option<String>, pack_format: Option<PackFormat>) -> Self {
        let mut manifest = Self::default();
        manifest.world.mc_version = mc_version;
        manifest.world.pack_format = pack_format;
        manifest
    }

    pub fn load(path: &Path) -> DppResult<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(DppError::ManifestMissing(path.to_path_buf()));
            }
            Err(source) => {
                return Err(DppError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        let manifest = toml::from_str::<Self>(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> DppResult<()> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(DppError::UnsupportedManifestSchema {
                found: self.schema,
                supported: MANIFEST_SCHEMA,
            });
        }
        Ok(())
    }

    pub fn to_toml(&self) -> DppResult<String> {
        Ok(toml::to_string(self)?)
    }

    pub fn write_new(&self, path: &Path) -> DppResult<()> {
        if path.exists() {
            return Err(DppError::ManifestExists(path.to_path_buf()));
        }
        write_new_atomic(path, self.to_toml()?.as_bytes())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mc_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_format: Option<PackFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderMode>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderMode {
    #[default]
    Prefix,
    LevelDat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSection {
    #[serde(default = "default_load_weight")]
    pub default_weight: u16,
}

impl Default for OrderSection {
    fn default() -> Self {
        Self {
            default_weight: DEFAULT_LOAD_WEIGHT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Version(String),
    Detailed(DependencyDetail),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyDetail {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
schema = 1

[world]
mc_version = "26.3"
pack_format = [121, 0]

[dependencies]
terralith = ">=2.5.0"
incendium = { version = ">=5.5.0", source = "modrinth" }

[order]
default_weight = 50
"#;

    #[test]
    fn parses_world_manifest() {
        let manifest: WorldManifest = toml::from_str(EXAMPLE).unwrap();
        manifest.validate().unwrap();

        assert_eq!(manifest.schema, 1);
        assert_eq!(manifest.world.mc_version.as_deref(), Some("26.3"));
        assert_eq!(manifest.world.pack_format, Some(PackFormat::new(121, 0)));
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(
            manifest.dependencies.get("terralith"),
            Some(&Dependency::Version(">=2.5.0".into()))
        );
        assert_eq!(
            manifest.dependencies.get("incendium"),
            Some(&Dependency::Detailed(DependencyDetail {
                version: ">=5.5.0".into(),
                source: Some("modrinth".into()),
            }))
        );
    }

    #[test]
    fn serializes_default_manifest_round_trip() {
        let manifest = WorldManifest::new(Some("26.3".into()), Some(PackFormat::new(121, 0)));
        let text = manifest.to_toml().unwrap();
        let parsed: WorldManifest = toml::from_str(&text).unwrap();

        assert_eq!(parsed, manifest);
        assert!(text.contains("schema = 1"));
        assert!(text.contains("[order]"));
    }

    #[test]
    fn rejects_future_schema() {
        let manifest = WorldManifest {
            schema: MANIFEST_SCHEMA + 1,
            ..WorldManifest::default()
        };
        assert!(manifest.validate().is_err());
    }
}
