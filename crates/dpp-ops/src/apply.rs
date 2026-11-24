use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{PreparedPackage, auto_snapshot, commit, list_snapshots, restore_snapshot};
use anyhow::{Context, Result, bail};
use dpp_core::{AppliedFile, IndexFile, LockFile, WorldManifest, WorldPaths, sha512_file};
use serde::{Deserialize, Serialize};

const PLAN_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub dry_run: bool,
    pub manual: bool,
    pub rcon: bool,
    pub server_root: Option<PathBuf>,
    pub log: Option<PathBuf>,
    pub abort: bool,
    pub rollback: bool,
}

#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub lock_hash: String,
    pub applied_at: String,
    pub method: String,
    pub verification: String,
    pub dry_run: bool,
    pub staged: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPlanStatus {
    pub transaction: String,
    pub packages: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StagedPlan {
    plan_version: u32,
    #[serde(default)]
    manifest: Option<WorldManifest>,
    lock: LockFile,
    index: IndexFile,
    #[serde(default, rename = "package")]
    packages: Vec<StagedPackage>,
    #[serde(default)]
    removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StagedPackage {
    install_name: String,
    staged_dir: String,
}

pub fn apply_world(world: &WorldPaths, options: ApplyOptions) -> Result<ApplyReport> {
    world.require_level_dat()?;
    world.require_state_dirs()?;

    if (options.abort || options.rollback)
        && (options.rcon || options.manual || options.log.is_some())
    {
        bail!("--abort/--rollback cannot be combined with activation options");
    }
    if options.abort && options.rollback {
        bail!("--abort and --rollback cannot be combined");
    }

    let discovered_plan = discover_plan(world)?;

    if options.abort {
        let Some((plan_dir, _)) = discovered_plan else {
            bail!("no staged plan to abort");
        };
        if options.dry_run {
            return Ok(report(
                current_lock_hash(world)?,
                "abort",
                "planned",
                true,
                false,
                Vec::new(),
            ));
        }
        fs::remove_dir_all(&plan_dir)
            .with_context(|| format!("failed to remove staged plan {}", plan_dir.display()))?;
        return Ok(report(
            current_lock_hash(world)?,
            "abort",
            "aborted",
            options.dry_run,
            false,
            Vec::new(),
        ));
    }

    if options.rollback {
        if discovered_plan.is_some() {
            bail!("a staged plan exists; apply or abort it before rollback");
        }
        let snapshot = list_snapshots(world)?
            .into_iter()
            .find(|snapshot| snapshot.name.starts_with("pre-apply-"))
            .context("no pre-apply snapshot is available")?;
        if options.dry_run {
            return Ok(report(
                current_lock_hash(world)?,
                "rollback",
                "planned",
                true,
                false,
                vec![format!("would restore snapshot {}", snapshot.name)],
            ));
        }
        restore_snapshot(world, &snapshot.name)?;
        let _ = fs::remove_file(world.applied());
        return Ok(report(
            current_lock_hash(world)?,
            "rollback",
            "restored",
            false,
            false,
            vec![format!("restored snapshot {}", snapshot.name)],
        ));
    }

    let mut staged = false;
    if let Some((plan_dir, plan)) = discovered_plan {
        if options.dry_run {
            return Ok(report(
                current_lock_hash(world)?,
                "next_start",
                "planned",
                true,
                false,
                vec![format!(
                    "would commit {} staged package(s)",
                    plan.packages.len()
                )],
            ));
        }
        auto_snapshot(world, "pre-apply")?;
        let (prepared, removed) = plan_prepared(&plan_dir, &plan)?;
        commit(
            world,
            &plan_dir,
            prepared,
            removed,
            plan.manifest.as_ref(),
            &plan.lock,
            &plan.index,
        )?;
        let _ = fs::remove_dir_all(&plan_dir);
        staged = true;
    }

    if !world.lock().is_file() {
        bail!("lock.toml is missing; run dpp install or dpp update first");
    }
    let lock = LockFile::load(&world.lock()).context("failed to read lock.toml")?;
    let index = IndexFile::load_optional(&world.index())?;
    if !lock.packages.is_empty() && index.is_none() {
        bail!("index.toml is missing; run dpp install first");
    }
    if options.rcon && options.manual {
        bail!("--rcon and --manual cannot be combined");
    }
    if options.log.is_some() && !options.rcon {
        bail!("--log requires --rcon");
    }

    let lock_hash = current_lock_hash(world)?;
    let log_offset = options.log.as_deref().map(log_offset).transpose()?;
    let mut warnings = Vec::new();

    let (method, verification) = if options.rcon {
        let root = options
            .server_root
            .as_deref()
            .context("--server-root is required for --rcon")?;
        if options.dry_run {
            ("rcon".to_owned(), "planned".to_owned())
        } else {
            let config = crate::rcon::config_from_server(root)?;
            let expected: Vec<String> = index
                .as_ref()
                .map(|index| {
                    index
                        .packages
                        .iter()
                        .map(|package| package.install_name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let reload = crate::rcon::reload_and_list(&config, &expected)?;
            for name in &reload.missing {
                warnings.push(format!(
                    "pack {name} is not in `datapack list enabled`; run /datapack enable \"file/{name}\""
                ));
            }
            if let (Some(log), Some(offset)) = (options.log.as_deref(), log_offset) {
                match check_log(log, offset) {
                    Ok(failures) if failures.is_empty() => {}
                    Ok(failures) => {
                        bail!(
                            "reload failed according to {}: {}",
                            log.display(),
                            failures.join("; ")
                        );
                    }
                    Err(error) => warnings.push(format!(
                        "could not verify reload log {}: {error:#}",
                        log.display()
                    )),
                }
            }
            (
                "rcon".to_owned(),
                if reload.missing.is_empty() {
                    "pack_list_ok".to_owned()
                } else {
                    "sent".to_owned()
                },
            )
        }
    } else if options.manual {
        ("manual".to_owned(), "unverified".to_owned())
    } else {
        ("next_start".to_owned(), "unverified".to_owned())
    };

    let applied_at = timestamp();
    let applied = AppliedFile {
        lock_hash: lock_hash.clone(),
        applied_at: applied_at.clone(),
        method: method.clone(),
        verification: verification.clone(),
    };
    if !options.dry_run {
        applied.write(&world.applied())?;
    }

    Ok(report(
        lock_hash,
        &method,
        &verification,
        options.dry_run,
        staged,
        warnings,
    ))
}

fn report(
    lock_hash: String,
    method: &str,
    verification: &str,
    dry_run: bool,
    staged: bool,
    warnings: Vec<String>,
) -> ApplyReport {
    ApplyReport {
        lock_hash,
        applied_at: timestamp(),
        method: method.to_owned(),
        verification: verification.to_owned(),
        dry_run,
        staged,
        warnings,
    }
}

fn discover_plan(world: &WorldPaths) -> Result<Option<(PathBuf, StagedPlan)>> {
    let staging = world.staging();
    if !staging.is_dir() {
        return Ok(None);
    }
    if staging.join("plan.toml").is_file() {
        bail!(
            "staged plan must be under {}/<txid>/plan.toml",
            staging.display()
        );
    }
    let mut plans = Vec::new();
    for entry in fs::read_dir(&staging).map_err(|source| dpp_core::DppError::Read {
        path: staging.to_path_buf(),
        source,
    })? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let plan_path = entry.path().join("plan.toml");
        if plan_path.is_file() {
            plans.push((entry.path(), plan_path));
        }
    }
    if plans.len() > 1 {
        bail!("multiple staged plans found; remove or abort all but one");
    }
    let Some((plan_dir, plan_path)) = plans.pop() else {
        return Ok(None);
    };
    let text = fs::read_to_string(&plan_path)
        .with_context(|| format!("failed to read {}", plan_path.display()))?;
    let plan: StagedPlan =
        toml::from_str(&text).map_err(|source| dpp_core::DppError::TomlRead {
            path: plan_path.clone(),
            source,
        })?;
    if plan.plan_version != PLAN_VERSION {
        bail!(
            "unsupported staged plan version {}; expected {}",
            plan.plan_version,
            PLAN_VERSION
        );
    }
    validate_plan_dir(&plan_dir, &plan)?;
    Ok(Some((plan_dir, plan)))
}

pub fn staged_plan_status(world: &WorldPaths) -> Result<Option<StagedPlanStatus>> {
    let Some((plan_dir, plan)) = discover_plan(world)? else {
        return Ok(None);
    };
    let transaction = plan_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_owned();
    Ok(Some(StagedPlanStatus {
        transaction,
        packages: plan.packages.len(),
        removed: plan.removed.len(),
    }))
}

fn validate_plan_dir(plan_dir: &Path, plan: &StagedPlan) -> Result<()> {
    let mut names = BTreeSet::new();
    for package in &plan.packages {
        if !names.insert(package.install_name.as_str()) {
            bail!(
                "duplicate install name in staged plan: {}",
                package.install_name
            );
        }
        let relative = Path::new(&package.staged_dir);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("unsafe staged_dir in plan: {}", package.staged_dir);
        }
        let path = plan_dir.join(relative);
        if !path.is_dir() {
            bail!("staged package directory is missing: {}", path.display());
        }
    }
    for name in &plan.removed {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            bail!("unsafe removed install name in plan: {name}");
        }
    }
    plan.lock.validate()?;
    plan.index.validate()?;
    for package in &plan.lock.packages {
        if !names.contains(package.install_name.as_str()) {
            bail!(
                "staged plan lock contains unstaged package {}",
                package.install_name
            );
        }
    }
    for package in &plan.index.packages {
        if !names.contains(package.install_name.as_str()) {
            bail!(
                "staged plan index contains unstaged package {}",
                package.install_name
            );
        }
    }
    for name in &plan.removed {
        if names.contains(name.as_str()) {
            bail!("staged package {name} is also listed as removed");
        }
    }
    Ok(())
}

fn plan_prepared(
    plan_dir: &Path,
    plan: &StagedPlan,
) -> Result<(Vec<PreparedPackage>, BTreeSet<String>)> {
    let mut prepared = Vec::new();
    for package in &plan.packages {
        let lock = plan
            .lock
            .packages
            .iter()
            .find(|lock| lock.install_name == package.install_name)
            .cloned()
            .with_context(|| {
                format!(
                    "staged plan has no lock package for {}",
                    package.install_name
                )
            })?;
        let index = plan
            .index
            .packages
            .iter()
            .find(|index| index.install_name == package.install_name)
            .cloned()
            .with_context(|| {
                format!(
                    "staged plan has no index package for {}",
                    package.install_name
                )
            })?;
        prepared.push(PreparedPackage {
            lock,
            index,
            manifest: None,
            staged_dir: plan_dir.join(&package.staged_dir),
        });
    }
    Ok((prepared, plan.removed.iter().cloned().collect()))
}

pub(crate) fn write_staged_plan(
    plan_dir: &Path,
    manifest: Option<&WorldManifest>,
    lock: &LockFile,
    index: &IndexFile,
    prepared: &[PreparedPackage],
    removed: &BTreeSet<String>,
) -> Result<PathBuf> {
    let mut packages = Vec::new();
    for package in prepared {
        let relative = package.staged_dir.strip_prefix(plan_dir).with_context(|| {
            format!(
                "staged directory {} is outside {}",
                package.staged_dir.display(),
                plan_dir.display()
            )
        })?;
        let staged_dir = relative
            .to_str()
            .context("staged directory is not valid UTF-8")?
            .to_owned();
        packages.push(StagedPackage {
            install_name: package.lock.install_name.clone(),
            staged_dir,
        });
    }
    let plan = StagedPlan {
        plan_version: PLAN_VERSION,
        manifest: manifest.cloned(),
        lock: lock.clone(),
        index: index.clone(),
        packages,
        removed: removed.iter().cloned().collect(),
    };
    let path = plan_dir.join("plan.toml");
    fs::write(
        &path,
        toml::to_string_pretty(&plan).context("failed to serialize staged plan")?,
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn current_lock_hash(world: &WorldPaths) -> Result<String> {
    if !world.lock().is_file() {
        return Ok(String::new());
    }
    Ok(format!("sha512:{}", sha512_file(&world.lock())?))
}

fn log_offset(path: &Path) -> Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).with_context(|| format!("failed to stat {}", path.display())),
    }
}

fn check_log(path: &Path, offset: u64) -> Result<Vec<String>> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let start = offset.min(bytes.len() as u64) as usize;
    let text = String::from_utf8_lossy(&bytes[start..]);
    let patterns = [
        "reload failed",
        "failed to reload",
        "error occurred while reloading",
        "failed to load",
    ];
    let failures = text
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            patterns.iter().any(|pattern| lower.contains(pattern))
        })
        .map(|line| line.trim().to_owned())
        .collect();
    Ok(failures)
}

fn timestamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "0".to_owned(),
        |duration| duration.as_secs().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use dpp_core::{DEFAULT_LOAD_WEIGHT, IndexPackage, LockPackage};

    #[test]
    fn detects_reload_failures_only_after_offset() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("latest.log");
        let prefix = "[old] Reload failed; keeping old data\n";
        fs::write(&path, prefix).unwrap();
        let offset = fs::metadata(&path).unwrap().len();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"[new] Reload failed; keeping old data\n[new] function ok\n")
            .unwrap();
        let failures = check_log(&path, offset).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("Reload failed"));
    }
    #[test]
    fn commits_staged_plan() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let world = WorldPaths::discover(Some(directory.path())).unwrap();
        world.ensure_state_dirs().unwrap();

        let plan_dir = world.staging().join("tx");
        let staged = plan_dir.join("0050_demo");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("pack.mcmeta"), b"{}").unwrap();

        let lock_package = LockPackage {
            id: "demo".to_owned(),
            package_id: None,
            source: "modrinth".to_owned(),
            project_id: None,
            version_id: None,
            version_number: "1.0.0".to_owned(),
            published_at: None,
            sha512: "a".repeat(128),
            sha1: None,
            size: None,
            url: None,
            file_name: None,
            manifest_hash: None,
            install_name: "0050_demo".to_owned(),
            load_weight: DEFAULT_LOAD_WEIGHT,
        };
        let index_package = IndexPackage {
            id: lock_package.id.clone(),
            package_id: None,
            source: lock_package.source.clone(),
            version_id: None,
            sha512: lock_package.sha512.clone(),
            install_name: lock_package.install_name.clone(),
            files: Vec::new(),
        };
        let plan = StagedPlan {
            plan_version: PLAN_VERSION,
            manifest: None,
            lock: LockFile {
                packages: vec![lock_package],
                ..LockFile::default()
            },
            index: IndexFile {
                packages: vec![index_package],
                ..IndexFile::default()
            },
            packages: vec![StagedPackage {
                install_name: "0050_demo".to_owned(),
                staged_dir: "0050_demo".to_owned(),
            }],
            removed: Vec::new(),
        };
        fs::write(
            plan_dir.join("plan.toml"),
            toml::to_string_pretty(&plan).unwrap(),
        )
        .unwrap();

        let report = apply_world(&world, ApplyOptions::default()).unwrap();
        assert!(report.staged);
        assert!(world.lock().is_file());
        assert!(world.index().is_file());
        assert!(
            world
                .root()
                .join("datapacks/0050_demo/pack.mcmeta")
                .is_file()
        );
        assert!(!plan_dir.exists());
        assert!(world.applied().is_file());
        let rollback = apply_world(
            &world,
            ApplyOptions {
                rollback: true,
                ..ApplyOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rollback.method, "rollback");
        assert_eq!(rollback.verification, "restored");
        assert!(!world.root().join("datapacks/0050_demo").exists());
        assert!(!world.applied().is_file());
    }
}
