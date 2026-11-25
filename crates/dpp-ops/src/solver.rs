use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use dpp_core::{Dependency, DppError, LockFile, PackFormat, PackManifest, WorldManifest};
use dpp_modrinth::{ModrinthClient, ProjectVersion, VersionFile};
use pubgrub::{
    Dependencies, DependencyConstraints, DependencyProvider, PackageResolutionStatistics, Ranges,
    resolve,
};
use semver::{Version, VersionReq};

use crate::ensure_cached;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct SolverError(String);

impl From<anyhow::Error> for SolverError {
    fn from(error: anyhow::Error) -> Self {
        Self(format!("{error:#}"))
    }
}

const ROOT_PACKAGE: &str = "__dpp_world__";

#[derive(Debug, Clone)]
pub struct SolvedPackage {
    pub source: String,
    pub version: ProjectVersion,
    pub file: VersionFile,
    pub archive: PathBuf,
    pub manifest: Option<PackManifest>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SolvePreferences {
    versions: BTreeMap<String, Version>,
    version_ids: BTreeMap<String, String>,
}

impl SolvePreferences {
    fn is_preferred(&self, package: &SolvedPackage) -> bool {
        self.version_ids
            .get(&package.source)
            .is_some_and(|version_id| version_id == &package.version.id)
            || self.versions.get(&package.source).is_some_and(|version| {
                Version::parse(&package.version.version_number)
                    .is_ok_and(|candidate| candidate == *version)
            })
    }
}

pub(crate) fn preferences_from_lock(lock: &LockFile) -> SolvePreferences {
    let mut preferences = SolvePreferences::default();
    for package in &lock.packages {
        if let Ok(version) = Version::parse(&package.version_number) {
            preferences.versions.insert(package.id.clone(), version);
        }
        if let Some(version_id) = &package.version_id {
            preferences
                .version_ids
                .insert(package.id.clone(), version_id.clone());
        }
    }
    preferences
}

#[derive(Clone)]
struct Candidate {
    version: Version,
    project: ProjectVersion,
    file: VersionFile,
}

#[derive(Clone)]
struct ResolvedCandidate {
    archive: PathBuf,
    manifest: Option<PackManifest>,
}

struct Provider<'a> {
    client: &'a ModrinthClient,
    mc_version: &'a str,
    target: PackFormat,
    direct: &'a BTreeMap<String, Dependency>,
    preferred: &'a SolvePreferences,
    excluded: &'a BTreeSet<(String, String)>,
    candidates: RefCell<BTreeMap<String, Vec<Candidate>>>,
    resolved: RefCell<BTreeMap<(String, Version), ResolvedCandidate>>,
}

pub fn solve_world(
    manifest: &WorldManifest,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
) -> Result<Vec<SolvedPackage>> {
    let preferences = SolvePreferences::default();
    solve_world_with_preferences(manifest, mc_version, target, client, &preferences)
}

pub(crate) fn solve_world_with_preferences(
    manifest: &WorldManifest,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    preferred: &SolvePreferences,
) -> Result<Vec<SolvedPackage>> {
    let mut attempts = 0;
    solve_with_exclusions(
        manifest,
        mc_version,
        target,
        client,
        preferred,
        &BTreeSet::new(),
        &mut attempts,
    )
}

const MAX_CONFLICT_RETRIES: usize = 16;

fn solve_with_exclusions(
    manifest: &WorldManifest,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    preferred: &SolvePreferences,
    excluded: &BTreeSet<(String, String)>,
    attempts: &mut usize,
) -> Result<Vec<SolvedPackage>> {
    *attempts += 1;
    if *attempts > MAX_CONFLICT_RETRIES {
        bail!("conflict resolution exceeded {MAX_CONFLICT_RETRIES} attempts");
    }
    let selected = solve_once(manifest, mc_version, target, client, preferred, excluded)?;
    let Some((left, right)) = find_conflict(&selected) else {
        return Ok(selected);
    };

    let left_key = (left.source.clone(), left.version.id.clone());
    let right_key = (right.source.clone(), right.version.id.clone());
    let left_preferred = preferred.is_preferred(left);
    let right_preferred = preferred.is_preferred(right);
    let choices = if left_preferred && !right_preferred {
        [right_key, left_key]
    } else {
        [left_key, right_key]
    };

    let mut last_error = None;
    for key in choices {
        if excluded.contains(&key) {
            continue;
        }
        let mut next = excluded.clone();
        next.insert(key);
        match solve_with_exclusions(
            manifest, mc_version, target, client, preferred, &next, attempts,
        ) {
            Ok(selected) => return Ok(selected),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow!(
            "unresolved conflict between {} and {}; no alternative candidate",
            left.source,
            right.source
        )
    }))
}

fn solve_once(
    manifest: &WorldManifest,
    mc_version: &str,
    target: PackFormat,
    client: &ModrinthClient,
    preferred: &SolvePreferences,
    excluded: &BTreeSet<(String, String)>,
) -> Result<Vec<SolvedPackage>> {
    let provider = Provider {
        client,
        mc_version,
        target,
        preferred,
        excluded,
        direct: &manifest.dependencies,
        candidates: RefCell::new(BTreeMap::new()),
        resolved: RefCell::new(BTreeMap::new()),
    };
    let root_version = Version::new(0, 0, 0);
    let solution = resolve(&provider, ROOT_PACKAGE.to_owned(), root_version)
        .map_err(|error| anyhow!("dependency resolution failed: {error:?}"))?;

    let mut selected = Vec::new();
    for (source, version) in solution.iter() {
        if source == ROOT_PACKAGE {
            continue;
        }
        let candidate = provider
            .find_candidate(source, version)?
            .with_context(|| format!("solver selected unavailable {source} {version}"))?;
        let resolved = provider.resolved_candidate(source, &candidate)?;
        selected.push(SolvedPackage {
            source: source.clone(),
            version: candidate.project,
            file: candidate.file,
            archive: resolved.archive,
            manifest: resolved.manifest,
        });
    }
    selected.sort_by(|left, right| left.source.cmp(&right.source));
    Ok(selected)
}

fn find_conflict(selected: &[SolvedPackage]) -> Option<(&SolvedPackage, &SolvedPackage)> {
    let mut aliases = BTreeMap::new();
    for package in selected {
        aliases.insert(package.source.as_str(), package);
        aliases.insert(package.version.project_id.as_str(), package);
        if let Some(manifest) = &package.manifest {
            aliases.insert(manifest.package.id.as_str(), package);
        }
    }

    for package in selected {
        let mut targets = BTreeSet::new();
        if let Some(manifest) = &package.manifest {
            targets.extend(manifest.conflicts.keys().map(String::as_str));
        }
        for dependency in &package.version.dependencies {
            if dependency.dependency_type == "incompatible"
                && let Some(project_id) = dependency.project_id.as_deref()
            {
                targets.insert(project_id);
            }
        }
        for target in targets {
            if let Some(other) = aliases.get(target)
                && other.source != package.source
            {
                return Some((package, other));
            }
        }
    }
    None
}

impl<'a> Provider<'a> {
    fn load_candidates(&self, source: &str) -> Result<Vec<Candidate>> {
        if let Some(candidates) = self.candidates.borrow().get(source) {
            return Ok(candidates.clone());
        }

        let versions = self
            .client
            .project_versions(source, Some(self.mc_version))
            .with_context(|| format!("failed to list Modrinth versions for {source}"))?;
        let mut unique = BTreeMap::new();
        let mut legacy = Vec::new();
        for project in versions {
            if !project.is_datapack() || !project.supports_game_version(self.mc_version) {
                continue;
            }
            let Some(file) = project.datapack_file().cloned() else {
                continue;
            };
            match Version::parse(&project.version_number) {
                Ok(version) => {
                    unique.entry(version.clone()).or_insert(Candidate {
                        version,
                        project,
                        file,
                    });
                }
                Err(_) => {
                    let version = Version::parse(&format!("0.0.0+legacy.{}", legacy.len()))
                        .expect("synthetic legacy version");
                    legacy.push(Candidate {
                        version,
                        project,
                        file,
                    });
                }
            }
        }
        let candidates: Vec<Candidate> = if unique.is_empty() {
            legacy.sort_by(|left, right| {
                right
                    .project
                    .date_published
                    .cmp(&left.project.date_published)
            });
            legacy
        } else {
            unique.into_values().rev().collect()
        };
        self.candidates
            .borrow_mut()
            .insert(source.to_owned(), candidates.clone());
        Ok(candidates)
    }

    fn version_set_for_req(&self, source: &str, requirement: &str) -> Result<Ranges<Version>> {
        let requirement = VersionReq::parse(requirement)
            .with_context(|| format!("invalid version requirement {requirement:?} for {source}"))?;
        let mut set = Ranges::empty();
        for candidate in self.load_candidates(source)? {
            if requirement.matches(&candidate.version) {
                set = set.union(&Ranges::singleton(candidate.version));
            }
        }
        Ok(set)
    }

    fn find_candidate(&self, source: &str, version: &Version) -> Result<Option<Candidate>> {
        Ok(self
            .load_candidates(source)?
            .into_iter()
            .find(|candidate| &candidate.version == version))
    }

    fn compatible_candidate(
        &self,
        source: &str,
        candidate: &Candidate,
    ) -> Result<Option<ResolvedCandidate>> {
        let key = (source.to_owned(), candidate.version.clone());
        if let Some(resolved) = self.resolved.borrow().get(&key) {
            return Ok(Some(resolved.clone()));
        }

        let archive = ensure_cached(self.client, &candidate.file)?;
        match dpp_core::inspect_zip(&archive, self.target) {
            Ok(_) => {}
            Err(DppError::PackIncompatible { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let manifest = PackManifest::load_from_zip(&archive)?;
        let resolved = ResolvedCandidate { archive, manifest };
        self.resolved.borrow_mut().insert(key, resolved.clone());
        Ok(Some(resolved))
    }

    fn resolved_candidate(&self, source: &str, candidate: &Candidate) -> Result<ResolvedCandidate> {
        self.compatible_candidate(source, candidate)?
            .with_context(|| {
                format!(
                    "{} {} does not support data pack format {}",
                    source, candidate.version, self.target
                )
            })
    }

    fn root_dependencies(&self) -> Result<DependencyConstraints<String, Ranges<Version>>> {
        let mut constraints = Vec::new();
        for (key, dependency) in self.direct {
            let source = dependency_source(key, dependency);
            let requirement = dependency_version(dependency);
            let set = self.version_set_for_req(&source, requirement)?;
            constraints.push((source, set));
        }
        Ok(constraints.into_iter().collect())
    }

    fn candidate_dependencies(
        &self,
        candidate: &Candidate,
        resolved: &ResolvedCandidate,
    ) -> Result<DependencyConstraints<String, Ranges<Version>>> {
        let mut constraints = Vec::new();
        if let Some(manifest) = &resolved.manifest {
            for (key, dependency) in &manifest.dependencies {
                let dependency_source = dependency_source(key, dependency);
                let requirement = dependency_version(dependency);
                let set = self.version_set_for_req(&dependency_source, requirement)?;
                constraints.push((dependency_source, set));
            }
            return Ok(constraints.into_iter().collect());
        }

        // Legacy packs have no in-pack dependency ranges. Use Modrinth's
        // required project dependencies as a conservative fallback.
        for dependency in &candidate.project.dependencies {
            if dependency.dependency_type != "required" {
                continue;
            }
            let Some(project_id) = dependency.project_id.as_deref() else {
                continue;
            };
            let set = self.version_set_for_req(project_id, "*")?;
            constraints.push((project_id.to_owned(), set));
        }
        Ok(constraints.into_iter().collect())
    }
}

impl DependencyProvider for Provider<'_> {
    type P = String;
    type V = Version;
    type VS = Ranges<Version>;
    type M = String;
    type Err = SolverError;
    type Priority = usize;

    fn prioritize(
        &self,
        _package: &Self::P,
        _range: &Self::VS,
        _statistics: &PackageResolutionStatistics,
    ) -> Self::Priority {
        0
    }

    fn choose_version(
        &self,
        package: &Self::P,
        range: &Self::VS,
    ) -> Result<Option<Self::V>, Self::Err> {
        if package == ROOT_PACKAGE {
            let root = Version::new(0, 0, 0);
            return Ok(range.contains(&root).then_some(root));
        }

        let candidates = self.load_candidates(package)?;
        if let Some(preferred) = self.preferred.versions.get(package)
            && let Some(candidate) = candidates.iter().find(|candidate| {
                &candidate.version == preferred
                    && range.contains(&candidate.version)
                    && !self
                        .excluded
                        .contains(&(package.clone(), candidate.project.id.clone()))
            })
            && self.compatible_candidate(package, candidate)?.is_some()
        {
            return Ok(Some(candidate.version.clone()));
        }
        if let Some(preferred) = self.preferred.version_ids.get(package)
            && let Some(candidate) = candidates.iter().find(|candidate| {
                candidate.project.id == *preferred
                    && range.contains(&candidate.version)
                    && !self
                        .excluded
                        .contains(&(package.clone(), candidate.project.id.clone()))
            })
            && self.compatible_candidate(package, candidate)?.is_some()
        {
            return Ok(Some(candidate.version.clone()));
        }
        for candidate in candidates {
            if !range.contains(&candidate.version)
                || self
                    .excluded
                    .contains(&(package.clone(), candidate.project.id.clone()))
            {
                continue;
            }
            if self.compatible_candidate(package, &candidate)?.is_some() {
                return Ok(Some(candidate.version));
            }
        }
        Ok(None)
    }

    fn get_dependencies(
        &self,
        package: &Self::P,
        version: &Self::V,
    ) -> Result<Dependencies<Self::P, Self::VS, Self::M>, Self::Err> {
        if package == ROOT_PACKAGE {
            return Ok(Dependencies::Available(self.root_dependencies()?));
        }

        let candidate = self
            .find_candidate(package, version)?
            .with_context(|| format!("candidate {package} {version} disappeared"))?;
        let resolved = self.resolved_candidate(package, &candidate)?;
        let dependencies = self.candidate_dependencies(&candidate, &resolved)?;
        Ok(Dependencies::Available(dependencies))
    }
}

fn dependency_source(key: &str, dependency: &Dependency) -> String {
    match dependency {
        Dependency::Detailed(detail)
            if detail
                .project
                .as_deref()
                .is_some_and(|project| !project.is_empty()) =>
        {
            detail.project.clone().expect("checked above")
        }
        _ => key.to_owned(),
    }
}

fn dependency_version(dependency: &Dependency) -> &str {
    match dependency {
        Dependency::Version(version) => version,
        Dependency::Detailed(detail) => &detail.version,
    }
}
