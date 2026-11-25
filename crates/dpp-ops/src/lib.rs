use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dpp_core::{
    DEFAULT_LOAD_WEIGHT, Dependency, DppError, ExtractedFile, HostProfile, IndexFile,
    IndexFileEntry, IndexPackage, LockFile, LockPackage, PackFormat, PackManifest, WorldManifest,
    WorldPaths, archive_cache_path, extract_zip, inspect_zip, sha512_file,
};
use dpp_modrinth::{Hashes, ModrinthClient, ProjectVersion, VersionFile};
use tempfile::TempDir;
mod apply;
mod config;
mod integrity;
mod journal;
mod order;
mod rcon;

pub use apply::{ApplyOptions, ApplyReport, StagedPlanStatus, apply_world, staged_plan_status};
pub use config::{
    ConfigApplyReport, ConfigPatch, PatchFormat, allow_target, apply_patch, target_lock_summary,
};
pub use integrity::{IntegrityReport, check_world_integrity};
pub use journal::{JournalStatus, recover as recover_world, status as journal_status};
pub use order::{OrderEntry, OrderReport, order_world};

mod snapshot;

pub use snapshot::{SnapshotInfo, auto_snapshot, list_snapshots, restore_snapshot, save_snapshot};
mod solver;

pub use solver::{SolvedPackage, solve_world};

#[cfg(test)]
mod solver_test;

#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub id: String,
    pub version: String,
    pub install_name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    pub installed: Vec<InstalledPackage>,
    pub skipped: Vec<String>,
    pub staged_plan: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateReport {
    pub installed: Vec<InstalledPackage>,
    pub skipped: Vec<String>,
    pub removed: Vec<String>,
    pub staged_plan: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AddReport {
    pub package: InstalledPackage,
    pub install: InstallReport,
}

struct SelectedVersion {
    project: String,
    version: ProjectVersion,
    file: VersionFile,
    archive: PathBuf,
}

pub(crate) struct PreparedPackage {
    pub(crate) lock: LockPackage,
    pub(crate) index: IndexPackage,
    pub(crate) manifest: Option<PackManifest>,
    pub(crate) staged_dir: PathBuf,
}

pub fn add_package(
    world: &WorldPaths,
    project: &str,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    host: Option<&HostProfile>,
) -> Result<AddReport> {
    world.require_level_dat()?;
    world.require_state_dirs()?;

    let mut manifest = WorldManifest::load(&world.manifest())?;
    ensure_manifest_target(&mut manifest, target, mc_version)?;
    manifest
        .dependencies
        .entry(project.to_owned())
        .or_insert_with(|| Dependency::Version("*".to_owned()));

    let index = IndexFile::load_optional(&world.index())?.unwrap_or_default();
    let existing_lock = LockFile::load_optional(&world.lock())?;
    let staging = create_staging(world)?;
    let mut prepared = Vec::new();
    let mut removed = BTreeSet::new();

    let (new_lock, mut new_index) = if let Some(mut lock) =
        existing_lock.filter(|lock| !lock.packages.is_empty())
    {
        check_lock_target(&lock, target, mc_version)?;
        validate_lock_packages(&lock)?;
        check_lock_covers_other_direct_dependencies(&manifest, &lock, project)?;

        let selected = resolve_project(client, project, mc_version, target)?;
        let install_name = choose_install_name(project, &lock, &index)?;
        let prepared_package = prepare_selected(&selected, &install_name, staging.path(), target)?;
        ensure_manifest_supported(prepared_package.manifest.as_ref(), host)?;

        lock.metadata.mc_version = Some(mc_version.to_owned());
        lock.metadata.pack_format = Some(target);
        lock.metadata.resolver = Some(format!("dpp {}", env!("CARGO_PKG_VERSION")));
        lock.packages
            .retain(|package| !(package.id == project && package.source == "modrinth"));
        lock.packages.push(prepared_package.lock.clone());

        if let Some(old) = index.find(project, "modrinth") {
            if old.install_name != install_name {
                removed.insert(old.install_name.clone());
            }
        }
        let mut new_index = index.clone();
        new_index
            .packages
            .retain(|package| !(package.id == project && package.source == "modrinth"));
        new_index.packages.push(prepared_package.index.clone());
        prepared.push(prepared_package);
        (lock, new_index)
    } else {
        let mut lock = LockFile::default();
        lock.metadata.mc_version = Some(mc_version.to_owned());
        lock.metadata.pack_format = Some(target);
        lock.metadata.resolver = Some(format!("dpp {}", env!("CARGO_PKG_VERSION")));

        for package in solve_world(&manifest, mc_version, target, client)? {
            ensure_host_preflight(package.manifest.as_ref(), host)?;
            let install_name = choose_install_name(&package.source, &lock, &index)?;
            let selected = SelectedVersion {
                project: package.source,
                version: package.version,
                file: package.file,
                archive: package.archive,
            };
            let prepared_package =
                prepare_selected(&selected, &install_name, staging.path(), target)?;
            lock.packages.push(prepared_package.lock.clone());
            prepared.push(prepared_package);
        }

        let desired = lock.clone();
        let mut new_index = prune_index(&index, &desired, &mut removed);
        for package in &prepared {
            new_index.packages.retain(|entry| {
                !(entry.id == package.lock.id && entry.source == package.lock.source)
            });
            new_index.packages.push(package.index.clone());
        }
        (lock, new_index)
    };

    sort_index(&mut new_index);
    let installed = commit(
        world,
        staging.path(),
        prepared,
        removed,
        Some(&manifest),
        &new_lock,
        &new_index,
    )?;
    let package = installed
        .iter()
        .find(|package| package.id == project)
        .cloned()
        .context("add produced no installed package")?;

    Ok(AddReport {
        install: InstallReport {
            installed,
            skipped: Vec::new(),
            staged_plan: None,
        },
        package,
    })
}

#[derive(Debug, Clone)]
pub struct RemoveReport {
    pub id: String,
    pub install_name: Option<String>,
    pub removed: Vec<String>,
    pub staged_plan: Option<PathBuf>,
}

pub fn remove_package(world: &WorldPaths, project: &str, stage: bool) -> Result<RemoveReport> {
    world.require_level_dat()?;
    world.require_state_dirs()?;

    let mut manifest = WorldManifest::load(&world.manifest())?;
    if manifest.dependencies.remove(project).is_none() {
        bail!("{project} is not a direct dependency");
    }
    if !stage {
        auto_snapshot(world, "pre-remove")?;
    }

    let mut lock = LockFile::load_optional(&world.lock())?.unwrap_or_default();
    let mut index = IndexFile::load_optional(&world.index())?.unwrap_or_default();
    validate_lock_packages(&lock)?;
    for package in &index.packages {
        validate_install_name(&package.install_name)?;
    }

    let mut install_name = None;
    for package in &lock.packages {
        if package.id == project {
            install_name.get_or_insert_with(|| package.install_name.clone());
        }
    }
    for package in &index.packages {
        if package.id == project {
            install_name.get_or_insert_with(|| package.install_name.clone());
        }
    }

    let reachable = reachable_install_names(world, &manifest, &lock)?;
    let mut managed = BTreeSet::new();
    for package in &lock.packages {
        managed.insert(package.install_name.clone());
    }
    for package in &index.packages {
        managed.insert(package.install_name.clone());
    }
    let removed: BTreeSet<String> = managed.difference(&reachable).cloned().collect();

    lock.packages
        .retain(|package| !removed.contains(&package.install_name));
    index
        .packages
        .retain(|package| !removed.contains(&package.install_name));
    let removed_names: Vec<String> = removed.iter().cloned().collect();

    let staging = create_staging(world)?;
    if stage {
        let plan_dir = staging.keep();
        let plan = crate::apply::write_staged_plan(
            &plan_dir,
            Some(&manifest),
            &lock,
            &index,
            &[],
            &removed,
        )?;
        return Ok(RemoveReport {
            id: project.to_owned(),
            install_name,
            removed: removed_names,
            staged_plan: Some(plan),
        });
    }
    commit(
        world,
        staging.path(),
        Vec::new(),
        removed,
        Some(&manifest),
        &lock,
        &index,
    )?;

    Ok(RemoveReport {
        id: project.to_owned(),
        install_name,
        removed: removed_names,
        staged_plan: None,
    })
}

fn find_lock_package<'a>(lock: &'a LockFile, dependency: &str) -> Option<&'a LockPackage> {
    lock.packages.iter().find(|package| {
        package.id == dependency
            || package.package_id.as_deref() == Some(dependency)
            || package.install_name == dependency
    })
}

fn reachable_install_names(
    world: &WorldPaths,
    manifest: &WorldManifest,
    lock: &LockFile,
) -> Result<BTreeSet<String>> {
    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::new();
    for dependency in manifest.dependencies.keys() {
        let package = find_lock_package(lock, dependency)
            .with_context(|| format!("lock is missing direct dependency {dependency}"))?;
        queue.push_back(package.install_name.clone());
    }

    while let Some(install_name) = queue.pop_front() {
        if !reachable.insert(install_name.clone()) {
            continue;
        }
        let Some(package) = lock
            .packages
            .iter()
            .find(|package| package.install_name == install_name)
        else {
            continue;
        };
        let Some(installed) = installed_pack_manifest(world, package)? else {
            continue;
        };
        for dependency in installed.dependencies.keys() {
            let package = find_lock_package(lock, dependency).with_context(|| {
                format!(
                    "cannot resolve dependency {dependency} declared by {install_name}; \
                     refusing to remove possible orphans"
                )
            })?;
            queue.push_back(package.install_name.clone());
        }
    }

    Ok(reachable)
}

pub fn install_world(
    world: &WorldPaths,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    host: Option<&HostProfile>,
    stage: bool,
) -> Result<InstallReport> {
    world.require_level_dat()?;
    world.require_state_dirs()?;

    let manifest = WorldManifest::load(&world.manifest())?;
    verify_manifest_target(&manifest, target, mc_version)?;
    let index = IndexFile::load_optional(&world.index())?.unwrap_or_default();
    let existing_lock = LockFile::load_optional(&world.lock())?;
    let staging = create_staging(world)?;
    let mut prepared = Vec::new();
    let mut skipped = Vec::new();
    let mut removed = BTreeSet::new();

    let (new_lock, new_index) = if let Some(mut lock) = existing_lock {
        check_lock_target(&lock, target, mc_version)?;
        validate_lock_packages(&lock)?;
        check_lock_has_direct_dependencies(&manifest, &lock)?;

        let mut new_index = prune_index(&index, &lock, &mut removed);
        for package in &lock.packages {
            if package_current(world, &index, package) {
                ensure_host_preflight(installed_pack_manifest(world, package)?.as_ref(), host)?;
                skipped.push(package.id.clone());
                continue;
            }

            collect_replaced_install_name(&index, package, &mut removed);
            let prepared_package = prepare_lock(package, staging.path(), target, client)?;
            ensure_host_preflight(prepared_package.manifest.as_ref(), host)?;
            new_index
                .packages
                .retain(|entry| !(entry.id == package.id && entry.source == package.source));
            new_index.packages.push(prepared_package.index.clone());
            prepared.push(prepared_package);
        }
        for prepared_package in &prepared {
            if let Some(package) = lock.packages.iter_mut().find(|package| {
                package.id == prepared_package.lock.id
                    && package.source == prepared_package.lock.source
            }) {
                if package.package_id.is_none() {
                    package.package_id = prepared_package.lock.package_id.clone();
                }
            }
        }

        if lock.metadata.pack_format.is_none() {
            lock.metadata.pack_format = Some(target);
        }
        if lock.metadata.mc_version.is_none() {
            lock.metadata.mc_version = Some(mc_version.to_owned());
        }
        sort_index(&mut new_index);
        (lock, new_index)
    } else {
        let mut lock = LockFile::default();
        lock.metadata.mc_version = Some(mc_version.to_owned());
        lock.metadata.pack_format = Some(target);
        lock.metadata.resolver = Some(format!("dpp {}", env!("CARGO_PKG_VERSION")));

        for package in solve_world(&manifest, mc_version, target, client)? {
            ensure_host_preflight(package.manifest.as_ref(), host)?;
            let install_name = choose_install_name(&package.source, &lock, &index)?;
            let selected = SelectedVersion {
                project: package.source,
                version: package.version,
                file: package.file,
                archive: package.archive,
            };
            let prepared_package =
                prepare_selected(&selected, &install_name, staging.path(), target)?;
            lock.packages.push(prepared_package.lock.clone());
            prepared.push(prepared_package);
        }

        let desired = lock.clone();
        let mut new_index = prune_index(&index, &desired, &mut removed);
        for package in &prepared {
            new_index.packages.retain(|entry| {
                !(entry.id == package.lock.id && entry.source == package.lock.source)
            });
            new_index.packages.push(package.index.clone());
        }
        sort_index(&mut new_index);
        (lock, new_index)
    };

    if stage {
        let plan_dir = staging.keep();
        let plan = crate::apply::write_staged_plan(
            &plan_dir, None, &new_lock, &new_index, &prepared, &removed,
        )?;
        return Ok(InstallReport {
            installed: Vec::new(),
            skipped,
            staged_plan: Some(plan),
        });
    }

    if prepared.is_empty() && removed.is_empty() {
        return Ok(InstallReport {
            installed: Vec::new(),
            skipped,
            staged_plan: None,
        });
    }

    let installed = commit(
        world,
        staging.path(),
        prepared,
        removed,
        None,
        &new_lock,
        &new_index,
    )?;

    Ok(InstallReport {
        installed,
        skipped,
        staged_plan: None,
    })
}

pub fn update_world(
    world: &WorldPaths,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    host: Option<&HostProfile>,
    stage: bool,
) -> Result<UpdateReport> {
    world.require_level_dat()?;
    world.require_state_dirs()?;

    let manifest = WorldManifest::load(&world.manifest())?;
    verify_manifest_target(&manifest, target, mc_version)?;

    let index = IndexFile::load_optional(&world.index())?.unwrap_or_default();
    let existing_lock = LockFile::load_optional(&world.lock())?;
    let old_lock = existing_lock.clone().unwrap_or_default();
    let staging = create_staging(world)?;
    let mut prepared = Vec::new();
    let mut skipped = Vec::new();

    let mut new_lock = LockFile::default();
    new_lock.metadata.mc_version = Some(mc_version.to_owned());
    new_lock.metadata.pack_format = Some(target);
    new_lock.metadata.resolver = Some(format!("dpp {}", env!("CARGO_PKG_VERSION")));

    for package in solve_world(&manifest, mc_version, target, client)? {
        ensure_host_preflight(package.manifest.as_ref(), host)?;
        let install_name = choose_install_name(&package.source, &old_lock, &index)?;
        let selected = SelectedVersion {
            project: package.source.clone(),
            version: package.version,
            file: package.file,
            archive: package.archive,
        };
        let prepared_package = prepare_selected(&selected, &install_name, staging.path(), target)?;
        ensure_host_preflight(prepared_package.manifest.as_ref(), host)?;

        let package_lock = prepared_package.lock.clone();
        if package_current(world, &index, &package_lock) {
            skipped.push(package.source);
        } else {
            prepared.push(prepared_package);
        }
        new_lock.packages.push(package_lock);
    }

    let mut removed = BTreeSet::new();
    let mut new_index = prune_index(&index, &new_lock, &mut removed);
    for package in &new_lock.packages {
        collect_replaced_install_name(&index, package, &mut removed);
    }
    for package in &prepared {
        new_index
            .packages
            .retain(|entry| !(entry.id == package.lock.id && entry.source == package.lock.source));
        new_index.packages.push(package.index.clone());
    }
    sort_index(&mut new_index);

    if stage {
        let plan_dir = staging.keep();
        let plan = crate::apply::write_staged_plan(
            &plan_dir, None, &new_lock, &new_index, &prepared, &removed,
        )?;
        return Ok(UpdateReport {
            installed: Vec::new(),
            skipped,
            removed: removed.into_iter().collect(),
            staged_plan: Some(plan),
        });
    }

    let lock_changed = existing_lock.as_ref() != Some(&new_lock);
    if prepared.is_empty() && removed.is_empty() && !lock_changed {
        return Ok(UpdateReport {
            installed: Vec::new(),
            skipped,
            removed: Vec::new(),
            staged_plan: None,
        });
    }

    auto_snapshot(world, "pre-update")?;
    let installed = commit(
        world,
        staging.path(),
        prepared,
        removed.clone(),
        None,
        &new_lock,
        &new_index,
    )?;

    Ok(UpdateReport {
        installed,
        skipped,
        removed: removed.into_iter().collect(),
        staged_plan: None,
    })
}

fn resolve_project(
    client: &ModrinthClient,
    project: &str,
    mc_version: &str,
    target: PackFormat,
) -> Result<SelectedVersion> {
    let versions = client
        .project_versions(project, Some(mc_version))
        .with_context(|| format!("failed to list Modrinth versions for {project}"))?;

    let mut candidates: Vec<ProjectVersion> = versions
        .into_iter()
        .filter(|version| {
            version.is_datapack()
                && version.supports_game_version(mc_version)
                && version.datapack_file().is_some()
        })
        .collect();
    candidates.sort_by(|left, right| {
        right
            .date_published
            .cmp(&left.date_published)
            .then_with(|| right.id.cmp(&left.id))
    });
    candidates.truncate(10);

    if candidates.is_empty() {
        bail!("no datapack version of {project} supports Minecraft {mc_version}");
    }

    for version in candidates {
        let file = version
            .datapack_file()
            .cloned()
            .context("datapack version has no zip file")?;
        let archive = ensure_cached(client, &file)?;
        match inspect_zip(&archive, target) {
            Ok(_) => {
                return Ok(SelectedVersion {
                    project: project.to_owned(),
                    version,
                    file,
                    archive,
                });
            }
            Err(DppError::PackIncompatible { .. }) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    bail!("no datapack version of {project} is compatible with data pack format {target}")
}

fn ensure_cached(client: &ModrinthClient, file: &VersionFile) -> Result<PathBuf> {
    let sha512 = validate_sha512(&file.hashes.sha512)?;
    let path = archive_cache_path(&sha512);
    if path.is_file() {
        if sha512_file(&path).is_ok_and(|actual| actual == sha512) {
            return Ok(path);
        }
        let _ = fs::remove_file(&path);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
    }
    client
        .download_file(file, &path)
        .with_context(|| format!("failed to download {}", file.url))?;
    Ok(path)
}

fn ensure_manifest_supported(
    manifest: Option<&PackManifest>,
    host: Option<&HostProfile>,
) -> Result<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    if !manifest.dependencies.is_empty() {
        bail!(
            "pack {} declares datapack dependencies, but this operation does not solve them",
            manifest.package.id
        );
    }
    if !manifest.conflicts.is_empty() {
        bail!(
            "pack {} declares conflicts, but this operation does not solve them",
            manifest.package.id
        );
    }
    ensure_host_preflight(Some(manifest), host)
}

fn ensure_host_preflight(
    manifest: Option<&PackManifest>,
    host: Option<&HostProfile>,
) -> Result<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    if manifest.host.requires.is_empty() && manifest.host.forbids.is_empty() {
        return Ok(());
    }
    let Some(host) = host else {
        bail!(
            "pack {} declares host requirements, but no server root was supplied; pass --server-root",
            manifest.package.id
        );
    };
    for capability in &manifest.host.requires {
        if !host.supports(capability) {
            bail!(
                "pack {} requires host capability {capability}, but detected host {} does not provide it",
                manifest.package.id,
                host.kind
            );
        }
    }
    for capability in &manifest.host.forbids {
        if host.supports(capability) {
            bail!(
                "pack {} forbids host capability {capability}, but detected host {} provides it",
                manifest.package.id,
                host.kind
            );
        }
    }
    Ok(())
}

fn prepare_selected(
    selected: &SelectedVersion,
    install_name: &str,
    staging: &Path,
    target: PackFormat,
) -> Result<PreparedPackage> {
    validate_install_name(install_name)?;
    inspect_zip(&selected.archive, target)?;
    let manifest = PackManifest::load_from_zip(&selected.archive)?;
    let staged_dir = staging.join(install_name);
    let files = extract_zip(&selected.archive, &staged_dir)?;
    let sha512 = validate_sha512(&selected.file.hashes.sha512)?;
    let manifest_hash = files
        .iter()
        .find(|file| file.path == "pack.mcmeta")
        .map(|file| format!("sha512:{}", file.sha512));

    let lock = LockPackage {
        id: selected.project.clone(),
        package_id: manifest
            .as_ref()
            .map(|manifest| manifest.package.id.clone()),
        source: "modrinth".to_owned(),
        project_id: Some(selected.version.project_id.clone()),
        version_id: Some(selected.version.id.clone()),
        version_number: selected.version.version_number.clone(),
        published_at: selected.version.date_published.clone(),
        sha512,
        sha1: selected.file.hashes.sha1.clone(),
        size: Some(selected.file.size),
        url: Some(selected.file.url.clone()),
        file_name: Some(selected.file.filename.clone()),
        manifest_hash,
        install_name: install_name.to_owned(),
        load_weight: DEFAULT_LOAD_WEIGHT,
    };
    let index = index_package(&lock, files);

    Ok(PreparedPackage {
        lock,
        index,
        manifest,
        staged_dir,
    })
}

fn prepare_lock(
    lock: &LockPackage,
    staging: &Path,
    target: PackFormat,
    client: &ModrinthClient,
) -> Result<PreparedPackage> {
    validate_install_name(&lock.install_name)?;
    let sha512 = validate_sha512(&lock.sha512)?;
    let url = lock
        .url
        .clone()
        .with_context(|| format!("lock package {} has no URL", lock.id))?;
    let file = VersionFile {
        filename: lock
            .file_name
            .clone()
            .unwrap_or_else(|| "pack.zip".to_owned()),
        hashes: Hashes {
            sha512: sha512.clone(),
            sha1: lock.sha1.clone(),
        },
        url,
        primary: true,
        size: lock.size.unwrap_or(0),
        file_type: None,
    };
    let archive = ensure_cached(client, &file)?;
    inspect_zip(&archive, target)?;
    let manifest = PackManifest::load_from_zip(&archive)?;
    if lock.package_id.is_some() && manifest.is_none() {
        bail!(
            "lock package {} declares a canonical id, but the pack has no dpp.toml",
            lock.id
        );
    }
    if let (Some(expected), Some(manifest)) = (&lock.package_id, &manifest) {
        if expected != &manifest.package.id {
            bail!(
                "lock package {} has canonical id {expected}, but pack declares {}",
                lock.id,
                manifest.package.id
            );
        }
    }

    let staged_dir = staging.join(&lock.install_name);
    let files = extract_zip(&archive, &staged_dir)?;
    let mut lock = lock.clone();
    if lock.package_id.is_none() {
        lock.package_id = manifest
            .as_ref()
            .map(|manifest| manifest.package.id.clone());
    }
    let index = index_package(&lock, files);

    Ok(PreparedPackage {
        lock,
        index,
        manifest,
        staged_dir,
    })
}

fn index_package(lock: &LockPackage, files: Vec<ExtractedFile>) -> IndexPackage {
    IndexPackage {
        id: lock.id.clone(),
        package_id: lock.package_id.clone(),
        source: lock.source.clone(),
        version_id: lock.version_id.clone(),
        sha512: lock.sha512.to_ascii_lowercase(),
        install_name: lock.install_name.clone(),
        files: files
            .into_iter()
            .map(|file| IndexFileEntry {
                path: file.path,
                sha512: file.sha512,
            })
            .collect(),
    }
}

pub(crate) fn commit(
    world: &WorldPaths,
    staging: &Path,
    prepared: Vec<PreparedPackage>,
    removed_names: BTreeSet<String>,
    manifest: Option<&WorldManifest>,
    lock: &LockFile,
    index: &IndexFile,
) -> Result<Vec<InstalledPackage>> {
    validate_lock_packages(lock)?;
    for package in &prepared {
        validate_install_name(&package.lock.install_name)?;
    }

    let datapacks = world.root().join("datapacks");
    fs::create_dir_all(&datapacks)
        .with_context(|| format!("failed to create {}", datapacks.display()))?;
    for package in &index.packages {
        validate_install_name(&package.install_name)?;
    }
    for name in &removed_names {
        validate_install_name(name)?;
    }

    let managed_names: BTreeSet<&str> = index
        .packages
        .iter()
        .map(|package| package.install_name.as_str())
        .collect();
    for package in &prepared {
        let target = datapacks.join(&package.lock.install_name);
        if target.exists() && !managed_names.contains(package.lock.install_name.as_str()) {
            bail!(
                "refusing to overwrite unmanaged datapack directory {}",
                target.display()
            );
        }
        let collision = index.packages.iter().any(|entry| {
            entry.install_name == package.lock.install_name
                && !(entry.id == package.lock.id && entry.source == package.lock.source)
        });
        if collision {
            bail!(
                "install name {} is already owned by another package",
                package.lock.install_name
            );
        }
    }

    let manifest_snapshot = snapshot(&world.manifest())?;
    let lock_snapshot = snapshot(&world.lock())?;
    let index_snapshot = snapshot(&world.index())?;

    let transaction_name = staging
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("txn")
        .to_owned();
    let trash = world.trash().join(&transaction_name);
    fs::create_dir_all(&trash).with_context(|| format!("failed to create {}", trash.display()))?;

    let mut backups = Vec::new();
    let mut installed_names = Vec::new();
    let backup_names: BTreeSet<String> = prepared
        .iter()
        .map(|package| package.lock.install_name.clone())
        .chain(removed_names.iter().cloned())
        .collect();
    let planned_installed: BTreeSet<String> = prepared
        .iter()
        .map(|package| package.lock.install_name.clone())
        .collect();
    let existing_names: BTreeSet<String> = backup_names
        .iter()
        .filter(|name| datapacks.join(name.as_str()).exists())
        .cloned()
        .collect();
    let mut journal = crate::journal::Journal::begin(
        world,
        &transaction_name,
        staging,
        &backup_names,
        &existing_names,
        &planned_installed,
        &removed_names,
    )?;
    journal.committing()?;

    let result = (|| -> Result<()> {
        for name in &backup_names {
            let target = datapacks.join(name);
            if target.exists() {
                let backup = trash.join(name);
                fs::rename(&target, &backup).with_context(|| {
                    format!("failed to move {} to transaction trash", target.display())
                })?;
                backups.push(name.clone());
            }
        }

        for package in &prepared {
            let target = datapacks.join(&package.lock.install_name);
            fs::rename(&package.staged_dir, &target)
                .with_context(|| format!("failed to install {}", package.lock.install_name))?;
            installed_names.push(package.lock.install_name.clone());
        }

        if let Some(manifest) = manifest {
            manifest.write(&world.manifest())?;
        }
        lock.write(&world.lock())?;
        index.write(&world.index())?;
        Ok(())
    })();

    if let Err(error) = result {
        for name in &installed_names {
            let _ = fs::remove_dir_all(datapacks.join(name));
        }
        for name in &backups {
            let _ = fs::rename(trash.join(name), datapacks.join(name));
        }
        let _ = restore_file(&world.manifest(), manifest_snapshot);
        let _ = restore_file(&world.lock(), lock_snapshot);
        let _ = restore_file(&world.index(), index_snapshot);
        journal.clear();
        let _ = fs::remove_dir_all(&trash);
        return Err(error);
    }

    journal.committed()?;
    let _ = fs::remove_dir_all(&trash);
    journal.clear();

    Ok(prepared
        .into_iter()
        .map(|package| InstalledPackage {
            id: package.lock.id,
            version: package.lock.version_number,
            install_name: package.lock.install_name.clone(),
            path: datapacks.join(package.lock.install_name),
        })
        .collect())
}

fn snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn restore_file(path: &Path, contents: Option<Vec<u8>>) -> Result<()> {
    match contents {
        Some(contents) => fs::write(path, contents)
            .with_context(|| format!("failed to restore {}", path.display())),
        None => {
            if path.exists() {
                fs::remove_file(path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
            Ok(())
        }
    }
}

fn ensure_manifest_target(
    manifest: &mut WorldManifest,
    target: PackFormat,
    mc_version: &str,
) -> Result<()> {
    if let Some(current) = manifest.world.pack_format {
        if current != target {
            bail!(
                "manifest targets data pack format {current}, but world is {target}; run dpp init/check"
            );
        }
    } else {
        manifest.world.pack_format = Some(target);
    }

    if let Some(current) = manifest.world.mc_version.as_deref() {
        if current != mc_version {
            bail!(
                "manifest targets Minecraft {current}, but world is {mc_version}; run dpp init/check"
            );
        }
    } else {
        manifest.world.mc_version = Some(mc_version.to_owned());
    }
    Ok(())
}

fn verify_manifest_target(
    manifest: &WorldManifest,
    target: PackFormat,
    mc_version: &str,
) -> Result<()> {
    if let Some(current) = manifest.world.pack_format {
        if current != target {
            bail!(
                "manifest targets data pack format {current}, but world is {target}; run dpp init/check"
            );
        }
    }
    if let Some(current) = manifest.world.mc_version.as_deref() {
        if current != mc_version {
            bail!(
                "manifest targets Minecraft {current}, but world is {mc_version}; run dpp init/check"
            );
        }
    }
    Ok(())
}

fn check_lock_target(lock: &LockFile, target: PackFormat, mc_version: &str) -> Result<()> {
    if lock.packages.is_empty() {
        return Ok(());
    }
    if let Some(current) = lock.metadata.pack_format {
        if current != target {
            bail!("lock targets data pack format {current}, but world is {target}");
        }
    }
    if let Some(current) = lock.metadata.mc_version.as_deref() {
        if current != mc_version {
            bail!("lock targets Minecraft {current}, but world is {mc_version}");
        }
    }
    Ok(())
}

fn check_lock_has_direct_dependencies(manifest: &WorldManifest, lock: &LockFile) -> Result<()> {
    let locked: BTreeSet<&str> = lock
        .packages
        .iter()
        .map(|package| package.id.as_str())
        .collect();
    for dependency in manifest.dependencies.keys() {
        if !locked.contains(dependency.as_str()) {
            bail!("lock is missing direct dependency {dependency}; run dpp add");
        }
    }
    Ok(())
}

fn check_lock_covers_other_direct_dependencies(
    manifest: &WorldManifest,
    lock: &LockFile,
    excluded: &str,
) -> Result<()> {
    let locked: BTreeSet<&str> = lock
        .packages
        .iter()
        .map(|package| package.id.as_str())
        .collect();
    for dependency in manifest.dependencies.keys() {
        if dependency.as_str() != excluded && !locked.contains(dependency.as_str()) {
            bail!("lock is missing direct dependency {dependency}; run dpp install");
        }
    }
    Ok(())
}

fn validate_lock_packages(lock: &LockFile) -> Result<()> {
    let mut names = BTreeSet::new();
    for package in &lock.packages {
        validate_install_name(&package.install_name)?;
        validate_sha512(&package.sha512)?;
        if !names.insert(package.install_name.clone()) {
            bail!(
                "lock contains duplicate install name {}",
                package.install_name
            );
        }
    }
    Ok(())
}

fn validate_install_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 100
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
        })
    {
        bail!("invalid install name {name:?}");
    }
    Ok(())
}

fn choose_install_name(project: &str, lock: &LockFile, index: &IndexFile) -> Result<String> {
    if let Some(package) = lock.packages.iter().find(|package| package.id == project) {
        validate_install_name(&package.install_name)?;
        return Ok(package.install_name.clone());
    }
    if let Some(package) = index.find(project, "modrinth") {
        validate_install_name(&package.install_name)?;
        return Ok(package.install_name.clone());
    }

    let name = make_install_name(project);
    let collision = lock
        .packages
        .iter()
        .any(|package| package.install_name == name && package.id != project)
        || index
            .packages
            .iter()
            .any(|package| package.install_name == name && package.id != project);
    if collision {
        bail!("install name {name} is already in use");
    }
    Ok(name)
}

fn make_install_name(project: &str) -> String {
    let base = project.rsplit('/').next().unwrap_or(project);
    let mut sanitized = String::with_capacity(base.len());
    for character in base.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    let sanitized: String = sanitized.chars().take(48).collect();
    if sanitized.is_empty() {
        format!("{DEFAULT_LOAD_WEIGHT:04}_pack")
    } else {
        format!("{DEFAULT_LOAD_WEIGHT:04}_{sanitized}")
    }
}

fn validate_sha512(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 128 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("invalid sha512 value {value:?}");
    }
    Ok(value)
}

fn package_current(world: &WorldPaths, index: &IndexFile, package: &LockPackage) -> bool {
    index
        .find(&package.id, &package.source)
        .is_some_and(|installed| {
            installed.install_name == package.install_name
                && installed.sha512.eq_ignore_ascii_case(&package.sha512)
                && world
                    .root()
                    .join("datapacks")
                    .join(&installed.install_name)
                    .is_dir()
        })
}

fn installed_pack_manifest(
    world: &WorldPaths,
    package: &LockPackage,
) -> Result<Option<PackManifest>> {
    let path = world
        .root()
        .join("datapacks")
        .join(&package.install_name)
        .join("dpp.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(PackManifest::from_toml_str(&text, &path)?))
}

fn collect_replaced_install_name(
    index: &IndexFile,
    package: &LockPackage,
    removed: &mut BTreeSet<String>,
) {
    if let Some(installed) = index.find(&package.id, &package.source) {
        if installed.install_name != package.install_name {
            removed.insert(installed.install_name.clone());
        }
    }
}

fn prune_index(index: &IndexFile, lock: &LockFile, removed: &mut BTreeSet<String>) -> IndexFile {
    let desired: BTreeSet<(&str, &str)> = lock
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package.source.as_str()))
        .collect();
    let mut new_index = IndexFile::default();
    for package in &index.packages {
        if desired.contains(&(package.id.as_str(), package.source.as_str())) {
            new_index.packages.push(package.clone());
        } else {
            removed.insert(package.install_name.clone());
        }
    }
    new_index
}

fn sort_index(index: &mut IndexFile) {
    index.packages.sort_by(|left, right| {
        left.install_name
            .cmp(&right.install_name)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn create_staging(world: &WorldPaths) -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix("txn-")
        .tempdir_in(world.staging())
        .with_context(|| {
            format!(
                "failed to create staging directory in {}",
                world.staging().display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dpp_core::HostKind;

    fn test_package(id: &str, install_name: &str) -> LockPackage {
        LockPackage {
            id: id.to_owned(),
            package_id: Some(id.to_owned()),
            source: "modrinth".to_owned(),
            project_id: None,
            version_id: None,
            version_number: "1.0.0".to_owned(),
            published_at: None,
            sha512: "a".repeat(128),
            sha1: None,
            size: Some(2),
            url: None,
            file_name: None,
            manifest_hash: None,
            install_name: install_name.to_owned(),
            load_weight: DEFAULT_LOAD_WEIGHT,
        }
    }

    fn write_installed_pack(
        world: &WorldPaths,
        install_name: &str,
        package_id: &str,
        dependencies: &[&str],
    ) {
        let directory = world.root().join("datapacks").join(install_name);
        std::fs::create_dir_all(&directory).unwrap();
        let mut text = format!("[package]\nid = \"{package_id}\"\nversion = \"1.0.0\"\n");
        if !dependencies.is_empty() {
            text.push_str("\n[dependencies]\n");
            for dependency in dependencies {
                text.push_str(&format!("{dependency} = \"*\"\n"));
            }
        }
        std::fs::write(directory.join("dpp.toml"), text).unwrap();
    }

    #[test]
    fn install_names_are_sanitized() {
        assert_eq!(make_install_name("my pack"), "0050_my_pack");
        assert_eq!(make_install_name("../../evil"), "0050_evil");
        validate_install_name("0050_pack").unwrap();
        assert!(validate_install_name("../evil").is_err());
        assert!(validate_install_name("a/b").is_err());
    }

    #[test]
    fn sha512_validation_is_strict() {
        assert!(validate_sha512("00").is_err());
        assert_eq!(validate_sha512(&"a".repeat(128)).unwrap(), "a".repeat(128));
    }

    #[test]
    fn transaction_installs_package_and_writes_state() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();

        let staging = create_staging(&paths).unwrap();
        let staged = staging.path().join("0050_demo");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("pack.mcmeta"), b"{}").unwrap();

        let package = LockPackage {
            id: "demo".to_owned(),
            package_id: None,
            source: "modrinth".to_owned(),
            project_id: None,
            version_id: None,
            version_number: "1.0.0".to_owned(),
            published_at: None,
            sha512: "a".repeat(128),
            sha1: None,
            size: Some(2),
            url: None,
            file_name: None,
            manifest_hash: None,
            install_name: "0050_demo".to_owned(),
            load_weight: DEFAULT_LOAD_WEIGHT,
        };
        let lock = LockFile {
            packages: vec![package.clone()],
            ..LockFile::default()
        };
        let index = IndexFile {
            packages: vec![index_package(&package, Vec::new())],
            ..IndexFile::default()
        };

        let installed = commit(
            &paths,
            staging.path(),
            vec![PreparedPackage {
                lock: package,
                index: index.packages[0].clone(),
                manifest: None,
                staged_dir: staged,
            }],
            BTreeSet::new(),
            None,
            &lock,
            &index,
        )
        .unwrap();

        assert_eq!(installed.len(), 1);
        assert!(
            paths
                .root()
                .join("datapacks/0050_demo/pack.mcmeta")
                .is_file()
        );
        assert!(LockFile::load_optional(&paths.lock()).unwrap().is_some());
        assert!(IndexFile::load_optional(&paths.index()).unwrap().is_some());
    }

    #[test]
    fn remove_deletes_managed_package_and_state() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();

        let mut manifest = WorldManifest::default();
        manifest
            .dependencies
            .insert("demo".to_owned(), Dependency::Version("*".to_owned()));
        manifest.write_new(&paths.manifest()).unwrap();

        let package = LockPackage {
            id: "demo".to_owned(),
            package_id: None,
            source: "modrinth".to_owned(),
            project_id: None,
            version_id: None,
            version_number: "1.0.0".to_owned(),
            published_at: None,
            sha512: "a".repeat(128),
            sha1: None,
            size: Some(2),
            url: None,
            file_name: None,
            manifest_hash: None,
            install_name: "0050_demo".to_owned(),
            load_weight: DEFAULT_LOAD_WEIGHT,
        };
        let lock = LockFile {
            packages: vec![package.clone()],
            ..LockFile::default()
        };
        lock.write(&paths.lock()).unwrap();
        let index = IndexFile {
            packages: vec![index_package(&package, Vec::new())],
            ..IndexFile::default()
        };
        index.write(&paths.index()).unwrap();

        let target = paths.root().join("datapacks/0050_demo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("pack.mcmeta"), b"{}").unwrap();

        let report = remove_package(&paths, "demo", false).unwrap();
        assert_eq!(report.install_name.as_deref(), Some("0050_demo"));
        assert!(!target.exists());
        assert!(
            WorldManifest::load(&paths.manifest())
                .unwrap()
                .dependencies
                .is_empty()
        );
        assert!(LockFile::load(&paths.lock()).unwrap().packages.is_empty());
        assert!(IndexFile::load(&paths.index()).unwrap().packages.is_empty());
        let snapshots = list_snapshots(&paths).unwrap();
        assert!(
            snapshots
                .iter()
                .any(|snapshot| snapshot.name.starts_with("pre-remove-"))
        );
    }

    #[test]
    fn remove_cleans_orphan_chain_and_stale_entries() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();

        let mut manifest = WorldManifest::default();
        manifest
            .dependencies
            .insert("root".to_owned(), Dependency::Version("*".to_owned()));
        manifest.write_new(&paths.manifest()).unwrap();

        let root = test_package("root", "0010_root");
        let child = test_package("child", "0020_child");
        let stale = test_package("stale", "0030_stale");
        let lock = LockFile {
            packages: vec![root.clone(), child.clone(), stale.clone()],
            ..LockFile::default()
        };
        lock.write(&paths.lock()).unwrap();
        let index = IndexFile {
            packages: vec![
                index_package(&root, Vec::new()),
                index_package(&child, Vec::new()),
                index_package(&stale, Vec::new()),
            ],
            ..IndexFile::default()
        };
        index.write(&paths.index()).unwrap();
        write_installed_pack(&paths, "0010_root", "root", &["child"]);
        write_installed_pack(&paths, "0020_child", "child", &[]);
        write_installed_pack(&paths, "0030_stale", "stale", &[]);

        let report = remove_package(&paths, "root", false).unwrap();
        assert_eq!(
            report.removed,
            vec!["0010_root", "0020_child", "0030_stale"]
        );
        assert!(LockFile::load(&paths.lock()).unwrap().packages.is_empty());
        assert!(IndexFile::load(&paths.index()).unwrap().packages.is_empty());
        for install_name in ["0010_root", "0020_child", "0030_stale"] {
            assert!(!paths.root().join("datapacks").join(install_name).exists());
        }
    }

    #[test]
    fn remove_keeps_shared_dependency() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();

        let mut manifest = WorldManifest::default();
        for dependency in ["root", "other"] {
            manifest
                .dependencies
                .insert(dependency.to_owned(), Dependency::Version("*".to_owned()));
        }
        manifest.write_new(&paths.manifest()).unwrap();

        let root = test_package("root", "0010_root");
        let shared = test_package("shared", "0020_shared");
        let other = test_package("other", "0030_other");
        let lock = LockFile {
            packages: vec![root.clone(), shared.clone(), other.clone()],
            ..LockFile::default()
        };
        lock.write(&paths.lock()).unwrap();
        let index = IndexFile {
            packages: vec![
                index_package(&root, Vec::new()),
                index_package(&shared, Vec::new()),
                index_package(&other, Vec::new()),
            ],
            ..IndexFile::default()
        };
        index.write(&paths.index()).unwrap();
        write_installed_pack(&paths, "0010_root", "root", &["shared"]);
        write_installed_pack(&paths, "0020_shared", "shared", &[]);
        write_installed_pack(&paths, "0030_other", "other", &["shared"]);

        let report = remove_package(&paths, "root", false).unwrap();
        assert_eq!(report.removed, vec!["0010_root"]);
        let lock = LockFile::load(&paths.lock()).unwrap();
        let remaining: BTreeSet<&str> = lock
            .packages
            .iter()
            .map(|package| package.id.as_str())
            .collect();
        assert_eq!(remaining, BTreeSet::from(["other", "shared"]));
        assert!(!paths.root().join("datapacks/0010_root").exists());
        assert!(paths.root().join("datapacks/0020_shared").is_dir());
        assert!(paths.root().join("datapacks/0030_other").is_dir());
    }

    #[test]
    fn remove_keeps_target_required_by_another_root() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let paths = WorldPaths::discover(Some(directory.path())).unwrap();
        paths.ensure_state_dirs().unwrap();

        let mut manifest = WorldManifest::default();
        for dependency in ["root", "other"] {
            manifest
                .dependencies
                .insert(dependency.to_owned(), Dependency::Version("*".to_owned()));
        }
        manifest.write_new(&paths.manifest()).unwrap();

        let root = test_package("root", "0010_root");
        let other = test_package("other", "0020_other");
        let lock = LockFile {
            packages: vec![root.clone(), other.clone()],
            ..LockFile::default()
        };
        lock.write(&paths.lock()).unwrap();
        let index = IndexFile {
            packages: vec![
                index_package(&root, Vec::new()),
                index_package(&other, Vec::new()),
            ],
            ..IndexFile::default()
        };
        index.write(&paths.index()).unwrap();
        write_installed_pack(&paths, "0010_root", "root", &[]);
        write_installed_pack(&paths, "0020_other", "other", &["root"]);

        let report = remove_package(&paths, "root", false).unwrap();
        assert!(report.removed.is_empty());
        assert!(paths.root().join("datapacks/0010_root").is_dir());
        assert!(paths.root().join("datapacks/0020_other").is_dir());
        let manifest = WorldManifest::load(&paths.manifest()).unwrap();
        assert_eq!(manifest.dependencies.len(), 1);
        assert!(manifest.dependencies.contains_key("other"));
    }

    #[test]
    fn rejects_unresolved_manifest_declarations() {
        let manifest = PackManifest::from_toml_str(
            r#"
[package]
id = "smithed:stdlib"
version = "0.3.1"

[dependencies]
"smithed:events" = ">=0.2.0"
"#,
            Path::new("demo.zip"),
        )
        .unwrap();
        let error = ensure_manifest_supported(Some(&manifest), None).unwrap_err();
        assert!(error.to_string().contains("dependencies"));
    }

    #[test]
    fn host_preflight_checks_kind_capabilities_and_forbids() {
        let manifest = PackManifest::from_toml_str(
            r#"
[package]
id = "test:hosted"
version = "1.0.0"

[host]
requires = ["paper", "paper.plugins/1"]
forbids = ["folia"]
"#,
            Path::new("demo.zip"),
        )
        .unwrap();
        let paper = HostProfile {
            kind: HostKind::Paper,
            capabilities: vec!["paper.plugins/1".to_owned()],
            ..HostProfile::default()
        };
        ensure_host_preflight(Some(&manifest), Some(&paper)).unwrap();

        let folia = HostProfile {
            kind: HostKind::Folia,
            ..HostProfile::default()
        };
        assert!(ensure_host_preflight(Some(&manifest), Some(&folia)).is_err());
        assert!(ensure_host_preflight(Some(&manifest), None).is_err());
    }
}
