use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use dpp_core::{IndexFile, LockFile, WorldManifest, WorldPaths};
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use super::validate_install_name;

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snapshot_version: u32,
    pub name: String,
    pub created_at: String,
    pub packages: Vec<String>,
    #[serde(default)]
    pub has_lock: bool,
    #[serde(default)]
    pub has_index: bool,
    #[serde(default)]
    pub has_manifest: bool,
}

#[derive(Debug, Clone)]
pub struct SnapshotInfo {
    pub name: String,
    pub created_at: String,
    pub packages: Vec<String>,
    pub path: PathBuf,
}

pub fn save_snapshot(world: &WorldPaths, name: &str) -> Result<SnapshotInfo> {
    validate_name(name)?;
    world.require_state_dirs()?;
    let target = world.snapshots().join(name);
    if target.exists() {
        bail!("snapshot already exists: {}", target.display());
    }

    let index = IndexFile::load_optional(&world.index())?;
    let packages: Vec<String> = index
        .as_ref()
        .map(|index| {
            index
                .packages
                .iter()
                .map(|package| package.install_name.clone())
                .collect()
        })
        .unwrap_or_default();
    let datapacks = world.root().join("datapacks");
    for package in &packages {
        validate_install_name(package)?;
    }
    for package in &packages {
        if !datapacks.join(package).is_dir() {
            bail!(
                "managed datapack directory is missing: {}",
                datapacks.join(package).display()
            );
        }
    }

    let temporary = Builder::new()
        .prefix(".snapshot-")
        .tempdir_in(world.snapshots())
        .context("failed to create temporary snapshot directory")?;
    let temporary_path = temporary.path().to_path_buf();
    let staged_datapacks = temporary_path.join("datapacks");
    fs::create_dir_all(&staged_datapacks)?;
    for package in &packages {
        copy_tree(&datapacks.join(package), &staged_datapacks.join(package))?;
    }

    let has_lock = world.lock().is_file();
    let has_index = world.index().is_file();
    let has_manifest = world.manifest().is_file();
    if has_lock {
        fs::copy(world.lock(), temporary_path.join("lock.toml"))
            .context("failed to copy lock.toml into snapshot")?;
    }
    if has_index {
        fs::copy(world.index(), temporary_path.join("index.toml"))
            .context("failed to copy index.toml into snapshot")?;
    }
    if has_manifest {
        fs::copy(world.manifest(), temporary_path.join("dpp.toml"))
            .context("failed to copy dpp.toml into snapshot")?;
    }

    let manifest = SnapshotManifest {
        snapshot_version: SNAPSHOT_VERSION,
        name: name.to_owned(),
        created_at: timestamp(),
        packages,
        has_lock,
        has_index,
        has_manifest,
    };
    fs::write(
        temporary_path.join("snapshot.toml"),
        toml::to_string_pretty(&manifest)?,
    )?;
    fs::rename(&temporary_path, &target)
        .with_context(|| format!("failed to publish snapshot {}", target.display()))?;
    std::mem::forget(temporary);

    Ok(info(name, &manifest, target))
}

pub fn auto_snapshot(world: &WorldPaths, prefix: &str) -> Result<SnapshotInfo> {
    save_snapshot(world, &format!("{prefix}-{}", timestamp_nanos()))
}

pub fn list_snapshots(world: &WorldPaths) -> Result<Vec<SnapshotInfo>> {
    world.require_state_dirs()?;
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(world.snapshots())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let manifest_path = path.join("snapshot.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = load_manifest(&path)?;
        snapshots.push(info(&manifest.name, &manifest, path));
    }
    snapshots.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(snapshots)
}

pub fn restore_snapshot(world: &WorldPaths, name: &str) -> Result<SnapshotInfo> {
    validate_name(name)?;
    world.require_state_dirs()?;
    let source = world.snapshots().join(name);
    let manifest = load_manifest(&source)?;
    let source_datapacks = source.join("datapacks");
    for package in &manifest.packages {
        validate_install_name(package)?;
    }
    for package in &manifest.packages {
        if !source_datapacks.join(package).is_dir() {
            bail!(
                "snapshot is missing managed datapack directory: {}",
                source_datapacks.join(package).display()
            );
        }
    }
    let snapshot_lock = if manifest.has_lock {
        Some(LockFile::load(&source.join("lock.toml"))?)
    } else {
        None
    };
    let snapshot_index = if manifest.has_index {
        Some(IndexFile::load(&source.join("index.toml"))?)
    } else {
        None
    };
    let snapshot_manifest = if manifest.has_manifest {
        Some(WorldManifest::load(&source.join("dpp.toml"))?)
    } else {
        None
    };

    let current_index = IndexFile::load_optional(&world.index())?;
    let current_names: Vec<String> = current_index
        .as_ref()
        .map(|index| {
            index
                .packages
                .iter()
                .map(|package| package.install_name.clone())
                .collect()
        })
        .unwrap_or_default();
    for name in &current_names {
        validate_install_name(name)?;
    }
    let datapacks = world.root().join("datapacks");
    fs::create_dir_all(&datapacks)?;
    for package in &manifest.packages {
        let target = datapacks.join(package);
        if target.exists() && !current_names.iter().any(|name| name == package) {
            bail!(
                "refusing to overwrite unmanaged datapack directory {}",
                target.display()
            );
        }
    }
    auto_snapshot(world, "pre-restore")?;

    let staging = Builder::new()
        .prefix(".restore-")
        .tempdir_in(world.staging())
        .context("failed to create restore staging directory")?;
    let staged_datapacks = staging.path().join("datapacks");
    fs::create_dir_all(&staged_datapacks)?;
    for package in &manifest.packages {
        copy_tree(
            &source_datapacks.join(package),
            &staged_datapacks.join(package),
        )?;
    }

    let lock_backup = read_optional(&world.lock())?;
    let manifest_backup = read_optional(&world.manifest())?;
    let index_backup = read_optional(&world.index())?;
    let transaction = world.trash().join(format!("restore-{}", timestamp_nanos()));
    fs::create_dir_all(&transaction)?;

    let mut moved = Vec::new();
    let mut installed = Vec::new();
    let result = (|| -> Result<()> {
        for package in &current_names {
            let target = datapacks.join(package);
            if target.exists() {
                let backup = transaction.join(package);
                fs::rename(&target, &backup)?;
                moved.push(package.clone());
            }
        }
        for package in &manifest.packages {
            let target = datapacks.join(package);
            fs::rename(staged_datapacks.join(package), &target)?;
            installed.push(package.clone());
        }
        if let Some(lock) = &snapshot_lock {
            lock.write(&world.lock())?;
        } else if world.lock().exists() {
            fs::remove_file(world.lock())?;
        }
        if let Some(index) = &snapshot_index {
            index.write(&world.index())?;
        } else if world.index().exists() {
            fs::remove_file(world.index())?;
        }
        if let Some(manifest) = &snapshot_manifest {
            manifest.write(&world.manifest())?;
        } else if world.manifest().exists() {
            fs::remove_file(world.manifest())?;
        }
        Ok(())
    })();

    if let Err(error) = result {
        for package in &installed {
            let _ = fs::remove_dir_all(datapacks.join(package));
        }
        for package in &moved {
            let _ = fs::rename(transaction.join(package), datapacks.join(package));
        }
        let _ = restore_file(&world.lock(), lock_backup);
        let _ = restore_file(&world.manifest(), manifest_backup);
        let _ = restore_file(&world.index(), index_backup);
        let _ = fs::remove_dir_all(&transaction);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&transaction);
    Ok(info(name, &manifest, source))
}

fn load_manifest(snapshot: &Path) -> Result<SnapshotManifest> {
    let path = snapshot.join("snapshot.toml");
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: SnapshotManifest =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    if manifest.snapshot_version != SNAPSHOT_VERSION {
        bail!(
            "unsupported snapshot schema {} in {}",
            manifest.snapshot_version,
            path.display()
        );
    }
    Ok(manifest)
}

fn info(name: &str, manifest: &SnapshotManifest, path: PathBuf) -> SnapshotInfo {
    SnapshotInfo {
        name: name.to_owned(),
        created_at: manifest.created_at.clone(),
        packages: manifest.packages.clone(),
        path,
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 100
        || name.contains('/')
        || name.contains('\\')
    {
        bail!("invalid snapshot name {name:?}");
    }
    Ok(())
}

fn timestamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "0".to_owned(),
        |duration| duration.as_secs().to_string(),
    )
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn restore_file(path: &Path, bytes: Option<Vec<u8>>) -> Result<()> {
    match bytes {
        Some(bytes) => {
            fs::write(path, bytes).with_context(|| format!("failed to restore {}", path.display()))
        }
        None => {
            if path.exists() {
                fs::remove_file(path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
            Ok(())
        }
    }
}

pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("refusing to snapshot symlink {}", source.display());
    }
    if metadata.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, target).with_context(|| format!("failed to copy {}", source.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dpp_core::{Dependency, IndexFile, IndexFileEntry, IndexPackage};

    #[test]
    fn snapshot_roundtrip_restores_datapacks_and_index() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let world = WorldPaths::discover(Some(directory.path())).unwrap();
        world.ensure_state_dirs().unwrap();

        let package_dir = world.root().join("datapacks/0050_demo");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("pack.mcmeta"), b"{}").unwrap();
        let index = IndexFile {
            packages: vec![IndexPackage {
                id: "demo".to_owned(),
                package_id: None,
                source: "modrinth".to_owned(),
                version_id: None,
                sha512: "a".repeat(128),
                install_name: "0050_demo".to_owned(),
                files: vec![IndexFileEntry {
                    path: "pack.mcmeta".to_owned(),
                    sha512: "b".repeat(128),
                }],
            }],
            ..IndexFile::default()
        };
        index.write(&world.index()).unwrap();
        let mut world_manifest = WorldManifest::default();
        world_manifest
            .dependencies
            .insert("demo".to_owned(), Dependency::Version("*".to_owned()));
        world_manifest.write(&world.manifest()).unwrap();

        let info = save_snapshot(&world, "before").unwrap();
        assert_eq!(info.packages, vec!["0050_demo"]);
        fs::write(package_dir.join("pack.mcmeta"), b"changed").unwrap();
        let restored = restore_snapshot(&world, "before").unwrap();
        assert_eq!(restored.name, "before");
        assert_eq!(fs::read(package_dir.join("pack.mcmeta")).unwrap(), b"{}");
        assert!(
            WorldManifest::load(&world.manifest())
                .unwrap()
                .dependencies
                .contains_key("demo")
        );
    }
}
