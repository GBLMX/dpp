use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DppError, DppResult};
use crate::paths::write_replace_atomic;

/// Paths owned by dpp at the server-instance scope.
#[derive(Debug, Clone)]
pub struct InstancePaths {
    root: PathBuf,
}

impl InstancePaths {
    pub fn discover(server_root: Option<&Path>) -> DppResult<Self> {
        let root = server_root.ok_or(DppError::ServerRootMissing)?;
        if !root.is_dir() {
            return Err(DppError::InstanceRootNotFound(root.to_path_buf()));
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".dpp")
    }

    pub fn instance_manifest(&self) -> PathBuf {
        self.state_dir().join("instance.toml")
    }

    pub fn host_lock(&self) -> PathBuf {
        self.state_dir().join("host.lock")
    }

    pub fn host_provided(&self) -> PathBuf {
        self.state_dir().join("host-provided.toml")
    }

    pub fn snapshots(&self) -> PathBuf {
        self.state_dir().join("snapshots")
    }

    pub fn ensure_state_dirs(&self) -> DppResult<()> {
        for dir in [self.state_dir(), self.snapshots()] {
            fs::create_dir_all(&dir).map_err(|source| DppError::CreateDir { path: dir, source })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProvidedPack {
    pub id: String,
    pub plugin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProvidedFile {
    #[serde(default, rename = "pack")]
    pub packs: Vec<HostProvidedPack>,
}

impl HostProvidedFile {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let file: Self = toml::from_str(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        for pack in &file.packs {
            if pack.id.is_empty() || pack.plugin.is_empty() || pack.id.len() > 200 {
                return Err(DppError::HostProvidedInvalid(path.to_path_buf()));
            }
        }
        Ok(file)
    }

    pub fn load_optional(path: &Path) -> DppResult<Self> {
        if path.is_file() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostLockFile {
    #[serde(default)]
    pub host: HostProfile,
    #[serde(default, rename = "provided_pack")]
    pub provided_packs: Vec<HostProvidedPack>,
}
impl HostLockFile {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })
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

    pub fn write(&self, path: &Path) -> DppResult<()> {
        let text = toml::to_string_pretty(self)?;
        write_replace_atomic(path, text.as_bytes())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    #[default]
    Unknown,
    Vanilla,
    Paper,
    Purpur,
    Folia,
    Modded,
}

impl std::fmt::Display for HostKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unknown => "unknown",
            Self::Vanilla => "vanilla",
            Self::Paper => "paper",
            Self::Purpur => "purpur",
            Self::Folia => "folia",
            Self::Modded => "modded",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostProfile {
    #[serde(default)]
    pub kind: HostKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adapters: Vec<AdapterObservation>,
}

impl HostProfile {
    pub fn supports(&self, token: &str) -> bool {
        token == self.kind.to_string()
            || self
                .capabilities
                .iter()
                .any(|capability| capability == token)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterObservation {
    pub name: String,
}

/// Best-effort offline host detection. This is deliberately not a claim that
/// the detected capabilities are enabled at runtime.
pub fn detect_host(root: &Path) -> DppResult<HostProfile> {
    if !root.is_dir() {
        return Err(DppError::InstanceRootNotFound(root.to_path_buf()));
    }

    let mut kind = HostKind::Unknown;
    let mut server_jar = None;
    let mut server_jar_seen = false;
    let entries = fs::read_dir(root).map_err(|source| DppError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DppError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".jar") {
            continue;
        }
        server_jar_seen = true;
        if !name.contains("dpp") && server_jar.is_none() {
            server_jar = Some(entry.path());
        }
        if name.contains("folia") {
            kind = HostKind::Folia;
        } else if name.contains("purpur") && kind != HostKind::Folia {
            kind = HostKind::Purpur;
        } else if name.contains("paper") && !matches!(kind, HostKind::Folia | HostKind::Purpur) {
            kind = HostKind::Paper;
        } else if kind == HostKind::Unknown && !name.contains("dpp") {
            kind = HostKind::Vanilla;
        }
    }

    let mods = root.join("mods");
    if mods.is_dir() {
        kind = HostKind::Modded;
    }

    let mut capabilities = Vec::new();
    if matches!(kind, HostKind::Paper | HostKind::Purpur | HostKind::Folia) {
        capabilities.push("paper.plugins/1".to_owned());
    }
    if root.join("kubejs").is_dir() {
        capabilities.push("kubejs.scripts/1".to_owned());
        if kind == HostKind::Unknown {
            kind = HostKind::Modded;
        }
    }

    let mut adapters = Vec::new();
    let plugins = root.join("plugins");
    if plugins.is_dir() {
        if let Ok(entries) = fs::read_dir(plugins) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".jar") && (lower.contains("dpp") || lower.contains("adapter")) {
                    adapters.push(AdapterObservation { name });
                }
            }
        }
    }
    capabilities.sort();
    capabilities.dedup();
    adapters.sort_by(|left, right| left.name.cmp(&right.name));
    let version = server_jar
        .as_deref()
        .and_then(|path| {
            crate::minecraft::read_version_json_from_jar(path)
                .ok()
                .flatten()
        })
        .and_then(|version| version.id);

    Ok(HostProfile {
        kind: if server_jar_seen || kind != HostKind::Unknown {
            kind
        } else {
            HostKind::Unknown
        },
        version,
        capabilities,
        adapters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_paper_host_and_adapters() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("paper-26.3.jar"), b"jar").unwrap();
        fs::create_dir(directory.path().join("kubejs")).unwrap();
        fs::create_dir(directory.path().join("plugins")).unwrap();
        fs::write(
            directory.path().join("plugins/dpp-adapter-paper.jar"),
            b"jar",
        )
        .unwrap();

        let host = detect_host(directory.path()).unwrap();
        assert_eq!(host.kind, HostKind::Paper);
        assert!(host.capabilities.contains(&"paper.plugins/1".to_owned()));
        assert!(host.capabilities.contains(&"kubejs.scripts/1".to_owned()));
        assert_eq!(host.adapters[0].name, "dpp-adapter-paper.jar");
    }

    #[test]
    fn instance_paths_and_host_lock_roundtrip() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("server.jar"), b"jar").unwrap();
        let paths = InstancePaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();
        assert!(paths.state_dir().is_dir());
        assert!(paths.snapshots().is_dir());

        let lock = HostLockFile {
            host: detect_host(directory.path()).unwrap(),
            ..HostLockFile::default()
        };
        lock.write(&paths.host_lock()).unwrap();
        let loaded = HostLockFile::load(&paths.host_lock()).unwrap();
        assert_eq!(loaded.host.kind, HostKind::Vanilla);
    }

    #[test]
    fn missing_server_root_is_rejected() {
        assert!(InstancePaths::discover(None).is_err());
        let directory = tempfile::tempdir().unwrap();
        assert!(InstancePaths::discover(Some(&directory.path().join("missing"))).is_err());
    }
}
