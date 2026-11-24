//! Modrinth API client for dpp.

pub mod client;
pub mod types;

pub use client::{ModrinthClient, ModrinthError};
pub use types::{Dependency, Hashes, ProjectVersion, SearchHit, SearchResponse, VersionFile};

/// The default User-Agent required by the Modrinth API.
pub fn default_user_agent() -> String {
    format!(
        "dpp/{} (https://github.com/GBLMX/dpp)",
        env!("CARGO_PKG_VERSION")
    )
}
