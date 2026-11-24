use std::io;
use std::path::PathBuf;

use crate::pack_format::PackFormat;

#[derive(Debug, thiserror::Error)]
pub enum DppError {
    #[error("world directory does not exist: {0}")]
    WorldNotFound(PathBuf),

    #[error("level.dat not found in world: {0}")]
    LevelDatMissing(PathBuf),

    #[error("dpp.toml already exists: {0}")]
    ManifestExists(PathBuf),

    #[error("dpp.toml not found in world: {0}")]
    ManifestMissing(PathBuf),

    #[error("dpp state directory is missing: {0}")]
    StateDirMissing(PathBuf),

    #[error("failed to determine current directory: {source}")]
    CurrentDir {
        #[source]
        source: io::Error,
    },

    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid TOML in {path}: {source}")]
    TomlRead {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to serialize TOML: {0}")]
    TomlWrite(#[from] toml::ser::Error),

    #[error("unsupported manifest schema {found}; this dpp supports schema {supported}")]
    UnsupportedManifestSchema { found: u32, supported: u32 },

    #[error("unsupported lock schema {found}; this dpp supports schema {supported}")]
    UnsupportedLockSchema { found: u32, supported: u32 },

    #[error("lock target pack format {lock} does not match manifest pack format {manifest}")]
    PackFormatMismatch {
        lock: PackFormat,
        manifest: PackFormat,
    },
}

pub type DppResult<T> = Result<T, DppError>;
