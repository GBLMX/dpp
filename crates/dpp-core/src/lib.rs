//! Core domain types and filesystem primitives for dpp.

pub mod applied;
pub mod error;
pub mod extract;
pub mod hash;
pub mod host;
pub mod index;
pub mod level_dat;
pub mod lock;
pub mod manifest;
pub mod minecraft;
pub mod pack_archive;
pub mod pack_build;
pub mod pack_format;
pub mod pack_manifest;
pub mod pack_meta;
pub mod paths;
pub mod protocol;

pub use applied::AppliedFile;
pub use error::{DppError, DppResult};
pub use extract::{ExtractedFile, extract_zip};
pub use hash::{sha512_bytes, sha512_file};
pub use host::{
    AdapterObservation, HostKind, HostLockFile, HostProfile, HostProvidedFile, HostProvidedPack,
    InstancePaths, detect_host,
};
pub use index::{INDEX_FILE_VERSION, IndexFile, IndexFileEntry, IndexPackage};
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
pub use pack_archive::{PackInspection, inspect_zip};
pub use pack_build::{BuildPackOptions, BuiltPack, build_pack};
pub use pack_format::{PackFormat, ParsePackFormatError};
pub use pack_manifest::{HostSection, PACK_MANIFEST_SCHEMA, PackManifest, PackageSection};
pub use pack_meta::{
    Features, Filter, FilterBlock, FormatValue, LegacyFormats, OverlayEntry, Overlays,
    PACK_META_FILE, PackMeta, PackSection,
};
pub use paths::{WorldPaths, archive_cache_path, cache_dir};
pub use protocol::{
    AdapterEvent, AdapterManifest, AdapterSection, AdapterSession, MAX_EVENT_BYTES,
    MAX_PAYLOAD_BYTES, PROTOCOL_ID, ProtocolError,
};
