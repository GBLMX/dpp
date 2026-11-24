use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DppError, DppResult};
use crate::paths::write_replace_atomic;

/// `.dpp/applied.toml` records the last activation action, not a running-state claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedFile {
    pub lock_hash: String,
    pub applied_at: String,
    pub method: String,
    pub verification: String,
}

impl AppliedFile {
    pub fn load(path: &Path) -> DppResult<Self> {
        let text = fs::read_to_string(path).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let applied = toml::from_str::<Self>(&text).map_err(|source| DppError::TomlRead {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(applied)
    }

    pub fn load_optional(path: &Path) -> DppResult<Option<Self>> {
        match Self::load(path) {
            Ok(applied) => Ok(Some(applied)),
            Err(DppError::Read { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn write(&self, path: &Path) -> DppResult<()> {
        let text = toml::to_string_pretty(self)?;
        write_replace_atomic(path, text.as_bytes())
    }
}
