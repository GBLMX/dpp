use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use dpp_core::{IndexFile, IndexPackage, LockFile, WorldPaths, sha512_file};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Verify the managed package layer against `index.toml`.
///
/// This is intentionally read-only: missing, changed, and extra files are
/// reported, never repaired. Foreign entries directly under `datapacks/` are
/// warnings because dpp does not own them.
pub fn check_world_integrity(
    world: &WorldPaths,
    lock: Option<&LockFile>,
    index: Option<&IndexFile>,
) -> Result<IntegrityReport> {
    let mut report = IntegrityReport::default();
    let lock_packages = lock.map(|lock| lock.packages.as_slice()).unwrap_or(&[]);
    let index_packages = index.map(|index| index.packages.as_slice()).unwrap_or(&[]);

    let mut known = BTreeSet::new();
    let mut indexed_names = BTreeSet::new();
    for package in index_packages {
        known.insert(package.install_name.clone());
        if !safe_relative_name(&package.install_name) {
            report.errors.push(format!(
                "invalid index install_name: {}",
                package.install_name
            ));
        }
        if !valid_sha512(&package.sha512) {
            report
                .errors
                .push(format!("invalid index sha512 for {}", package.id));
        }
        if !indexed_names.insert(package.install_name.clone()) {
            report.errors.push(format!(
                "duplicate index install_name: {}",
                package.install_name
            ));
        }
    }
    let mut locked_names = BTreeSet::new();
    for package in lock_packages {
        known.insert(package.install_name.clone());
        if !safe_relative_name(&package.install_name) {
            report.errors.push(format!(
                "invalid lock install_name: {}",
                package.install_name
            ));
        }
        if !valid_sha512(&package.sha512) {
            report
                .errors
                .push(format!("invalid lock sha512 for {}", package.id));
        }
        if !locked_names.insert(package.install_name.clone()) {
            report.errors.push(format!(
                "duplicate managed install_name: {}",
                package.install_name
            ));
        }
        match index {
            Some(index) => match index.find(&package.id, &package.source) {
                Some(installed) => {
                    if installed.install_name != package.install_name {
                        report.errors.push(format!(
                            "lock/index install_name mismatch for {}: {} vs {}",
                            package.id, package.install_name, installed.install_name
                        ));
                    }
                    if !installed.sha512.eq_ignore_ascii_case(&package.sha512) {
                        report.errors.push(format!(
                            "lock/index sha512 mismatch for {} ({})",
                            package.id, package.install_name
                        ));
                    }
                }
                None => report.errors.push(format!(
                    "lock package {} ({}) is missing from index.toml",
                    package.id, package.install_name
                )),
            },
            None => report.errors.push(format!(
                "lock package {} ({}) has no index.toml",
                package.id, package.install_name
            )),
        }
    }
    for package in index_packages {
        let in_lock = lock_packages
            .iter()
            .any(|locked| locked.id == package.id && locked.source == package.source);
        if !in_lock {
            report.errors.push(format!(
                "index package {} ({}) is missing from lock.toml",
                package.id, package.install_name
            ));
        }
    }

    for package in index_packages {
        check_package(world, package, &mut report)?;
    }

    let datapacks = world.root().join("datapacks");
    match fs::metadata(&datapacks) {
        Ok(metadata) if metadata.is_dir() => {
            for entry in fs::read_dir(&datapacks).with_context(|| {
                format!("failed to read datapacks directory {}", datapacks.display())
            })? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !known.contains(&name) {
                    report
                        .warnings
                        .push(format!("foreign datapacks entry: {name}"));
                }
            }
        }
        Ok(_) => report.warnings.push(format!(
            "datapacks path is not a directory: {}",
            datapacks.display()
        )),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(source).with_context(|| {
                format!("failed to inspect datapacks path {}", datapacks.display())
            });
        }
    }

    Ok(report)
}

fn check_package(
    world: &WorldPaths,
    package: &IndexPackage,
    report: &mut IntegrityReport,
) -> Result<()> {
    if !safe_relative_name(&package.install_name) {
        report.errors.push(format!(
            "invalid managed install_name: {}",
            package.install_name
        ));
        return Ok(());
    }
    let root = world.root().join("datapacks").join(&package.install_name);
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            report
                .errors
                .push(format!("missing package directory: {}", root.display()));
            return Ok(());
        }
        Err(source) => {
            return Err(source).with_context(|| format!("failed to inspect {}", root.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        report.errors.push(format!(
            "symlink not allowed as package root: {}",
            root.display()
        ));
        return Ok(());
    }
    if !metadata.is_dir() {
        report.errors.push(format!(
            "package path is not a directory: {}",
            root.display()
        ));
        return Ok(());
    }

    let mut indexed = BTreeMap::new();
    for file in &package.files {
        let Some(relative) = safe_relative_path(&file.path) else {
            report
                .errors
                .push(format!("invalid indexed file path: {}", file.path));
            continue;
        };
        let key = normalized(&relative);
        if indexed.insert(key.clone(), file).is_some() {
            report
                .errors
                .push(format!("duplicate indexed file path: {key}"));
            continue;
        }
        if !valid_sha512(&file.sha512) {
            report
                .errors
                .push(format!("invalid sha512 for indexed file: {key}"));
            continue;
        }
        let path = root.join(&relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                report
                    .errors
                    .push(format!("missing file: {}/{}", package.install_name, key));
                continue;
            }
            Err(source) => {
                return Err(source)
                    .with_context(|| format!("failed to inspect {}", path.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            report.errors.push(format!(
                "symlink not allowed in managed package: {}/{}",
                package.install_name, key
            ));
            continue;
        }
        if !metadata.is_file() {
            report
                .errors
                .push(format!("not a file: {}/{}", package.install_name, key));
            continue;
        }
        match sha512_file(&path) {
            Ok(actual) if actual.eq_ignore_ascii_case(&file.sha512) => {}
            Ok(_) => report
                .errors
                .push(format!("sha512 mismatch: {}/{}", package.install_name, key)),
            Err(error) => report.errors.push(format!(
                "cannot hash {}/{}: {error}",
                package.install_name, key
            )),
        }
    }

    let mut actual_files = BTreeSet::new();
    collect_files(&root, &root, &mut actual_files)?;
    for relative in actual_files {
        if !indexed.contains_key(&relative) {
            report.errors.push(format!(
                "unexpected file: {}/{}",
                package.install_name, relative
            ));
        }
    }
    Ok(())
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to read directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.insert(normalized(relative));
        }
    }
    Ok(())
}

fn valid_sha512(value: &str) -> bool {
    value.len() == 128 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn safe_relative_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

fn safe_relative_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

fn normalized(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dpp_core::{DEFAULT_LOAD_WEIGHT, IndexFileEntry, LockPackage, sha512_bytes};

    fn fixture(content: &[u8]) -> (tempfile::TempDir, WorldPaths, IndexFile, LockFile) {
        let directory = tempfile::tempdir().unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        let root = paths.root().join("datapacks/demo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pack.mcmeta"), content).unwrap();

        let archive_hash = "a".repeat(128);
        let lock = LockFile {
            packages: vec![LockPackage {
                id: "demo".to_owned(),
                package_id: Some("demo".to_owned()),
                source: "modrinth".to_owned(),
                project_id: None,
                version_id: None,
                version_number: "1.0.0".to_owned(),
                published_at: None,
                sha512: archive_hash.clone(),
                sha1: None,
                size: Some(content.len() as u64),
                url: None,
                file_name: None,
                manifest_hash: None,
                install_name: "demo".to_owned(),
                load_weight: DEFAULT_LOAD_WEIGHT,
            }],
            ..LockFile::default()
        };
        let index = IndexFile {
            packages: vec![IndexPackage {
                id: "demo".to_owned(),
                package_id: Some("demo".to_owned()),
                source: "modrinth".to_owned(),
                version_id: None,
                sha512: archive_hash,
                install_name: "demo".to_owned(),
                files: vec![IndexFileEntry {
                    path: "pack.mcmeta".to_owned(),
                    sha512: sha512_bytes(content),
                }],
            }],
            ..IndexFile::default()
        };
        (directory, paths, index, lock)
    }

    #[test]
    fn accepts_matching_package_and_reports_file_drift() {
        let (_directory, paths, index, lock) = fixture(b"ok");
        let report = check_world_integrity(&paths, Some(&lock), Some(&index)).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let file = paths.root().join("datapacks/demo/pack.mcmeta");
        std::fs::write(&file, b"changed").unwrap();
        let report = check_world_integrity(&paths, Some(&lock), Some(&index)).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("sha512 mismatch"))
        );

        std::fs::write(paths.root().join("datapacks/demo/extra.txt"), b"x").unwrap();
        let report = check_world_integrity(&paths, Some(&lock), Some(&index)).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("unexpected file"))
        );

        std::fs::remove_file(&file).unwrap();
        let report = check_world_integrity(&paths, Some(&lock), Some(&index)).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("missing file"))
        );
    }

    #[test]
    fn reports_foreign_datapacks_as_warning() {
        let (_directory, paths, index, lock) = fixture(b"ok");
        std::fs::create_dir_all(paths.root().join("datapacks/foreign")).unwrap();
        let report = check_world_integrity(&paths, Some(&lock), Some(&index)).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("foreign datapacks entry")),
            "{:?}",
            report.warnings
        );
    }
}
