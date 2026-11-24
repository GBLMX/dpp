use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use crate::error::{DppError, DppResult};

/// Paths owned by dpp inside a single Minecraft world.
#[derive(Debug, Clone)]
pub struct WorldPaths {
    root: PathBuf,
}

impl WorldPaths {
    /// Discover a world directory, defaulting to the current directory.
    pub fn discover(world: Option<&Path>) -> DppResult<Self> {
        let root = match world {
            Some(path) => path.to_path_buf(),
            None => std::env::current_dir().map_err(|source| DppError::CurrentDir { source })?,
        };

        if !root.is_dir() {
            return Err(DppError::WorldNotFound(root));
        }

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn level_dat(&self) -> PathBuf {
        self.root.join("level.dat")
    }

    pub fn session_lock(&self) -> PathBuf {
        self.root.join("session.lock")
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join("dpp.toml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join(".dpp")
    }

    pub fn lock(&self) -> PathBuf {
        self.state_dir().join("lock.toml")
    }

    pub fn index(&self) -> PathBuf {
        self.state_dir().join("index.toml")
    }

    pub fn applied(&self) -> PathBuf {
        self.state_dir().join("applied.toml")
    }

    pub fn journal(&self) -> PathBuf {
        self.state_dir().join("journal.toml")
    }

    pub fn staging(&self) -> PathBuf {
        self.state_dir().join("staging")
    }

    pub fn snapshots(&self) -> PathBuf {
        self.state_dir().join("snapshots")
    }

    pub fn trash(&self) -> PathBuf {
        self.state_dir().join("trash")
    }

    pub fn require_level_dat(&self) -> DppResult<()> {
        if self.level_dat().is_file() {
            Ok(())
        } else {
            Err(DppError::LevelDatMissing(self.level_dat()))
        }
    }

    pub fn ensure_state_dirs(&self) -> DppResult<()> {
        for dir in [
            self.state_dir(),
            self.staging(),
            self.snapshots(),
            self.trash(),
        ] {
            fs::create_dir_all(&dir).map_err(|source| DppError::CreateDir { path: dir, source })?;
        }
        Ok(())
    }

    pub fn require_state_dirs(&self) -> DppResult<()> {
        for dir in [
            self.state_dir(),
            self.staging(),
            self.snapshots(),
            self.trash(),
        ] {
            if !dir.is_dir() {
                return Err(DppError::StateDirMissing(dir));
            }
        }
        Ok(())
    }
}

/// Write a new file atomically without replacing an existing path.
pub(crate) fn write_new_atomic(path: &Path, contents: &[u8]) -> DppResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| DppError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    temporary
        .write_all(contents)
        .map_err(|source| DppError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| DppError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| DppError::Write {
            path: path.to_path_buf(),
            source: error.error,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_explicit_world_and_creates_state_dirs() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("level.dat"), b"test").unwrap();

        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.require_level_dat().unwrap();
        paths.ensure_state_dirs().unwrap();
        paths.require_state_dirs().unwrap();

        assert!(paths.staging().is_dir());
        assert!(paths.snapshots().is_dir());
        assert!(paths.trash().is_dir());
    }

    #[test]
    fn missing_world_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        assert!(WorldPaths::discover(Some(&missing)).is_err());
    }
}
