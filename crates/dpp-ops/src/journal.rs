use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::snapshot::copy_tree;
use anyhow::{Context, Result, bail};
use dpp_core::WorldPaths;
use serde::{Deserialize, Serialize};

const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Committing,
    Committed,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committing => "committing",
            Self::Committed => "committed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStatus {
    pub transaction: String,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalFile {
    journal_version: u32,
    transaction: String,
    phase: Phase,
    staging: String,
    backup_names: Vec<String>,
    existing_names: Vec<String>,
    installed_names: Vec<String>,
    removed_names: Vec<String>,
}

pub(crate) struct Journal {
    path: PathBuf,
    file: JournalFile,
}

impl Journal {
    pub(crate) fn begin(
        world: &WorldPaths,
        transaction: &str,
        staging: &Path,
        backup_names: &BTreeSet<String>,
        existing_names: &BTreeSet<String>,
        installed_names: &BTreeSet<String>,
        removed_names: &BTreeSet<String>,
    ) -> Result<Self> {
        let trash = world.trash().join(transaction);
        let state = trash.join("state");
        fs::create_dir_all(&state)
            .with_context(|| format!("failed to create {}", state.display()))?;
        copy_optional(&world.manifest(), &state.join("dpp.toml"))?;
        copy_optional(&world.lock(), &state.join("lock.toml"))?;
        copy_optional(&world.index(), &state.join("index.toml"))?;

        let file = JournalFile {
            journal_version: JOURNAL_VERSION,
            transaction: transaction.to_owned(),
            phase: Phase::Prepared,
            staging: staging.to_string_lossy().into_owned(),
            backup_names: backup_names.iter().cloned().collect(),
            existing_names: existing_names.iter().cloned().collect(),
            installed_names: installed_names.iter().cloned().collect(),
            removed_names: removed_names.iter().cloned().collect(),
        };
        let journal = Self {
            path: world.journal(),
            file,
        };
        journal.write()?;
        Ok(journal)
    }

    fn set_phase(&mut self, phase: Phase) -> Result<()> {
        self.file.phase = phase;
        self.write()
    }
    pub(crate) fn committing(&mut self) -> Result<()> {
        self.set_phase(Phase::Committing)
    }

    pub(crate) fn committed(&mut self) -> Result<()> {
        self.set_phase(Phase::Committed)
    }

    pub(crate) fn clear(self) {
        let _ = fs::remove_file(&self.path);
    }

    fn write(&self) -> Result<()> {
        let text = toml::to_string_pretty(&self.file).context("failed to serialize journal")?;
        write_atomic(&self.path, text.as_bytes())
    }
}

pub fn status(world: &WorldPaths) -> Result<Option<JournalStatus>> {
    let path = world.journal();
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let file: JournalFile =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    if file.journal_version != JOURNAL_VERSION {
        bail!(
            "unsupported journal version {}; expected {}",
            file.journal_version,
            JOURNAL_VERSION
        );
    }
    Ok(Some(JournalStatus {
        transaction: file.transaction,
        phase: file.phase.label().to_owned(),
    }))
}

struct RecoveryReport {
    transaction: String,
    phase: Phase,
}

pub fn recover(world: &WorldPaths) -> Result<Option<String>> {
    let journal_path = world.journal();
    if !journal_path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&journal_path)
        .with_context(|| format!("failed to read {}", journal_path.display()))?;
    let file: JournalFile = toml::from_str(&text)
        .with_context(|| format!("failed to parse {}", journal_path.display()))?;
    if file.journal_version != JOURNAL_VERSION {
        bail!(
            "unsupported journal version {}; expected {}",
            file.journal_version,
            JOURNAL_VERSION
        );
    }
    let report = RecoveryReport {
        transaction: file.transaction.clone(),
        phase: file.phase,
    };
    let trash = world.trash().join(&file.transaction);
    let staging = PathBuf::from(&file.staging);
    match file.phase {
        Phase::Committed => {
            remove_staging(world, &staging);
            let _ = fs::remove_dir_all(&trash);
        }
        Phase::Prepared => {
            remove_staging(world, &staging);
            let _ = fs::remove_dir_all(&trash);
        }
        Phase::Committing => {
            rollback(
                world,
                &trash,
                &file.existing_names,
                &file.installed_names,
                &file.removed_names,
            )?;
            restore_state(world, &trash.join("state"))?;
            remove_staging(world, &staging);
            let _ = fs::remove_dir_all(&trash);
        }
    }
    fs::remove_file(&journal_path)
        .with_context(|| format!("failed to remove {}", journal_path.display()))?;
    Ok(Some(format!(
        "recovered transaction {} (phase {:?})",
        report.transaction, report.phase
    )))
}

fn rollback(
    world: &WorldPaths,
    trash: &Path,
    existing_names: &[String],
    installed_names: &[String],
    removed_names: &[String],
) -> Result<()> {
    let datapacks = world.root().join("datapacks");
    fs::create_dir_all(&datapacks)?;
    for name in installed_names {
        let target = datapacks.join(name);
        let backup = trash.join(name);
        if backup.exists() {
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("failed to remove {}", target.display()))?;
            }
            copy_tree(&backup, &target)?;
        } else if target.exists() && !existing_names.contains(name) {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to remove {}", target.display()))?;
        }
    }
    for name in removed_names {
        let target = datapacks.join(name);
        let backup = trash.join(name);
        if backup.exists() {
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("failed to remove {}", target.display()))?;
            }
            copy_tree(&backup, &target)?;
        }
    }
    Ok(())
}

fn restore_state(world: &WorldPaths, state: &Path) -> Result<()> {
    restore_file(&state.join("dpp.toml"), &world.manifest())?;
    restore_file(&state.join("lock.toml"), &world.lock())?;
    restore_file(&state.join("index.toml"), &world.index())?;
    Ok(())
}

fn restore_file(backup: &Path, target: &Path) -> Result<()> {
    if backup.is_file() {
        fs::copy(backup, target)
            .with_context(|| format!("failed to restore {}", target.display()))?;
    } else if target.exists() {
        fs::remove_file(target)
            .with_context(|| format!("failed to remove {}", target.display()))?;
    }
    Ok(())
}

fn copy_optional(source: &Path, target: &Path) -> Result<()> {
    if source.is_file() {
        fs::copy(source, target).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

fn remove_staging(world: &WorldPaths, staging: &Path) {
    if staging.starts_with(world.staging()) {
        let _ = fs::remove_dir_all(staging);
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync journal")?;
    temporary
        .persist(path)
        .map_err(|error| anyhow::anyhow!("failed to publish journal: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_committing_transaction() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("level.dat"), b"fixture").unwrap();
        let world = WorldPaths::discover(Some(directory.path())).unwrap();
        world.ensure_state_dirs().unwrap();
        fs::write(world.manifest(), b"old manifest").unwrap();
        fs::write(world.lock(), b"old lock").unwrap();
        fs::write(world.index(), b"old index").unwrap();
        fs::write(world.manifest(), b"new manifest").unwrap();

        let trash = world.trash().join("tx");
        fs::create_dir_all(&trash).unwrap();
        fs::create_dir_all(trash.join("state")).unwrap();
        std::fs::write(trash.join("state/dpp.toml"), b"old manifest").unwrap();
        std::fs::write(trash.join("state/lock.toml"), b"old lock").unwrap();
        std::fs::write(trash.join("state/index.toml"), b"old index").unwrap();
        fs::create_dir_all(world.root().join("datapacks/0050_demo")).unwrap();
        let journal = JournalFile {
            journal_version: JOURNAL_VERSION,
            transaction: "tx".to_owned(),
            phase: Phase::Committing,
            staging: world.staging().join("tx").to_string_lossy().into_owned(),
            backup_names: vec!["0050_demo".to_owned()],
            existing_names: Vec::new(),
            installed_names: vec!["0050_demo".to_owned()],
            removed_names: Vec::new(),
        };
        fs::write(world.journal(), toml::to_string_pretty(&journal).unwrap()).unwrap();

        recover(&world).unwrap();
        assert!(!world.journal().exists());
        assert!(!world.root().join("datapacks/0050_demo").exists());
        assert_eq!(fs::read(world.manifest()).unwrap(), b"old manifest");
    }
}
