use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::error::{DppError, DppResult};
use crate::pack_format::PackFormat;

pub const PACK_META_FILE: &str = "pack.mcmeta";

/// Parsed `pack.mcmeta`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PackMeta {
    pub pack: PackSection,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PackSection {
    #[serde(default)]
    pub description: Option<Value>,
    #[serde(default, rename = "pack_format")]
    pub legacy_format: Option<u32>,
    #[serde(default, rename = "supported_formats")]
    pub legacy_supported_formats: Option<LegacyFormats>,
    #[serde(default, rename = "min_format")]
    pub min_format: Option<FormatValue>,
    #[serde(default, rename = "max_format")]
    pub max_format: Option<FormatValue>,
    #[serde(default)]
    pub overlays: Option<Overlays>,
    #[serde(default)]
    pub features: Option<Features>,
    #[serde(default)]
    pub filter: Option<Filter>,
}

impl PackMeta {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json_str(&text).map_err(|source| DppError::PackMetaJson {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Check whether this pack declares support for a data pack format.
    pub fn supports(&self, format: PackFormat) -> bool {
        if format.major() < 82 {
            self.pack.supports_legacy(format)
        } else {
            self.pack.supports_new(format)
        }
    }

    pub fn feature_flags(&self) -> &[String] {
        self.pack
            .features
            .as_ref()
            .map_or(&[], |features| features.enabled.as_slice())
    }
}

impl PackSection {
    fn supports_legacy(&self, format: PackFormat) -> bool {
        let major = format.major();
        if let Some(supported) = self.legacy_supported_formats {
            let (min, max) = supported.range();
            return (min..=max).contains(&major);
        }
        self.legacy_format == Some(major)
    }

    fn supports_new(&self, format: PackFormat) -> bool {
        let min = match self.min_format {
            Some(value) => value.min(),
            None => return false,
        };
        let max = match self.max_format {
            Some(value) => value.max(),
            None => return false,
        };

        if format < min || format.major() > max.major {
            return false;
        }
        if format.major() < max.major {
            return true;
        }
        max.any_minor || format.minor() <= max.minor
    }
}

/// A `min_format`/`max_format` entry: integer or one/two-element array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum FormatValue {
    Single(u32),
    One([u32; 1]),
    Pair([u32; 2]),
}

impl FormatValue {
    fn min(self) -> PackFormat {
        match self {
            Self::Single(major) | Self::One([major]) => PackFormat::new(major, 0),
            Self::Pair([major, minor]) => PackFormat::new(major, minor),
        }
    }

    fn max(self) -> FormatBound {
        match self {
            // A single integer (or one-element list) for max_format means any minor.
            Self::Single(major) | Self::One([major]) => FormatBound {
                major,
                minor: u32::MAX,
                any_minor: true,
            },
            Self::Pair([major, minor]) => FormatBound {
                major,
                minor,
                any_minor: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FormatBound {
    major: u32,
    minor: u32,
    any_minor: bool,
}

/// Legacy `supported_formats`: integer, one/two-element list, or inclusive object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum LegacyFormats {
    Single(u32),
    One([u32; 1]),
    Pair([u32; 2]),
    Range {
        min_inclusive: u32,
        max_inclusive: u32,
    },
}

impl LegacyFormats {
    fn range(self) -> (u32, u32) {
        match self {
            Self::Single(major) | Self::One([major]) => (major, major),
            Self::Pair([min, max]) => (min, max),
            Self::Range {
                min_inclusive,
                max_inclusive,
            } => (min_inclusive, max_inclusive),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Overlays {
    #[serde(default)]
    pub entries: Vec<OverlayEntry>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OverlayEntry {
    pub directory: String,
    #[serde(default)]
    pub min_format: Option<FormatValue>,
    #[serde(default)]
    pub max_format: Option<FormatValue>,
    #[serde(default)]
    pub formats: Option<LegacyFormats>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Features {
    #[serde(default)]
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Filter {
    #[serde(default)]
    pub block: Vec<FilterBlock>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FilterBlock {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = r#"{
        "pack": {
            "description": "example",
            "min_format": [121, 0],
            "max_format": [121, 0]
        }
    }"#;

    #[test]
    fn supports_current_format_range() {
        let meta = PackMeta::from_json_str(CURRENT).unwrap();

        assert!(meta.supports(PackFormat::new(121, 0)));
        assert!(!meta.supports(PackFormat::new(121, 1)));
        assert!(!meta.supports(PackFormat::new(120, 0)));
    }

    #[test]
    fn single_max_format_accepts_any_minor() {
        let meta = PackMeta::from_json_str(
            r#"{"pack":{"description":"x","min_format":88,"max_format":88}}"#,
        )
        .unwrap();

        assert!(meta.supports(PackFormat::new(88, 0)));
        assert!(meta.supports(PackFormat::new(88, 7)));
        assert!(!meta.supports(PackFormat::new(87, 0)));
        assert!(!meta.supports(PackFormat::new(89, 0)));
    }

    #[test]
    fn accepts_one_element_format_lists() {
        let meta = PackMeta::from_json_str(
            r#"{"pack":{"description":"x","min_format":[88],"max_format":[88]}}"#,
        )
        .unwrap();
        assert!(meta.supports(PackFormat::new(88, 0)));
        assert!(meta.supports(PackFormat::new(88, 9)));
        assert!(!meta.supports(PackFormat::new(89, 0)));

        let legacy = PackMeta::from_json_str(
            r#"{"pack":{"description":"x","pack_format":48,"supported_formats":[48]}}"#,
        )
        .unwrap();
        assert!(legacy.supports(PackFormat::new(48, 0)));
        assert!(!legacy.supports(PackFormat::new(49, 0)));
    }

    #[test]
    fn supports_legacy_supported_formats() {
        let meta = PackMeta::from_json_str(
            r#"{
                "pack": {
                    "description": "legacy",
                    "pack_format": 48,
                    "supported_formats": {"min_inclusive": 48, "max_inclusive": 61}
                }
            }"#,
        )
        .unwrap();

        assert!(meta.supports(PackFormat::new(48, 0)));
        assert!(meta.supports(PackFormat::new(61, 5)));
        assert!(!meta.supports(PackFormat::new(47, 0)));
        assert!(!meta.supports(PackFormat::new(71, 0)));
    }

    #[test]
    fn supports_legacy_pack_format_exact_major() {
        let meta = PackMeta::from_json_str(r#"{"pack":{"description":"legacy","pack_format":4}}"#)
            .unwrap();

        assert!(meta.supports(PackFormat::new(4, 0)));
        assert!(!meta.supports(PackFormat::new(5, 0)));
    }

    #[test]
    fn new_range_requires_both_bounds() {
        let meta = PackMeta::from_json_str(
            r#"{"pack":{"description":"x","min_format":[82,0],"max_format":[82,1]}}"#,
        )
        .unwrap();

        assert!(meta.supports(PackFormat::new(82, 1)));
        assert!(!meta.supports(PackFormat::new(82, 2)));
        assert!(!meta.supports(PackFormat::new(81, 0)));
    }

    #[test]
    fn parses_features_filter_and_overlays() {
        let meta = PackMeta::from_json_str(
            r#"{
                "pack": {
                    "description": {"text": "x"},
                    "pack_format": 48,
                    "features": {"enabled": ["minecraft:update_1_21"]},
                    "filter": {"block": [{"namespace": "minecraft", "path": "recipe/.*"}]},
                    "overlays": {"entries": [{
                        "directory": "overlay_1",
                        "formats": {"min_inclusive": 57, "max_inclusive": 61}
                    }]}
                }
            }"#,
        )
        .unwrap();

        assert_eq!(meta.feature_flags(), ["minecraft:update_1_21"]);
        assert_eq!(meta.pack.filter.unwrap().block.len(), 1);
        let overlays = meta.pack.overlays.unwrap();
        assert_eq!(overlays.entries[0].directory, "overlay_1");
    }

    #[test]
    fn loads_from_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PACK_META_FILE);
        fs::write(&path, CURRENT).unwrap();

        assert!(
            PackMeta::load(&path)
                .unwrap()
                .supports(PackFormat::new(121, 0))
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(PackMeta::from_json_str("not json").is_err());
    }
}
