//! Core domain types and filesystem primitives for dpp.

pub mod error;
pub mod level_dat;
pub mod lock;
pub mod manifest;
pub mod minecraft;
pub mod pack_format;
pub mod pack_meta;
pub mod paths;

pub use error::{DppError, DppResult};
pub use level_dat::LevelInfo;
pub use lock::{LOCK_FILE_VERSION, LockFile, LockMetadata, LockPackage};
pub use manifest::{
    DEFAULT_LOAD_WEIGHT, Dependency, DependencyDetail, MANIFEST_SCHEMA, OrderMode, OrderSection,
    WorldManifest, WorldSection,
};
pub use minecraft::{
    JarVersion, adjacent_server_jar, detect_pack_format, find_version_jar, minecraft_dir,
    pack_format_for_release, read_version_json_from_jar,
};
pub use pack_format::{PackFormat, ParsePackFormatError};
pub use pack_meta::{
    Features, Filter, FilterBlock, FormatValue, LegacyFormats, OverlayEntry, Overlays,
    PACK_META_FILE, PackMeta, PackSection,
};
pub use paths::WorldPaths;
