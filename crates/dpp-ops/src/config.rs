use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use dpp_core::{InstancePaths, sha512_bytes};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const ALLOWLIST_FILE: &str = "config-allowlist.toml";
const LOCK_FILE: &str = "config.lock.toml";
const BACKUP_DIR: &str = "config-backups";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchFormat {
    Toml,
    Json,
}

impl PatchFormat {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "toml" => Ok(Self::Toml),
            "json" => Ok(Self::Json),
            other => bail!("unsupported config format {other:?}; use toml or json"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MergeMode {
    Merge,
    Replace,
}

#[derive(Debug)]
pub struct ConfigPatch {
    pub target: String,
    pub format: PatchFormat,
    pub package: Option<String>,
    merge: MergeMode,
    data: PatchData,
}

#[derive(Debug)]
enum PatchData {
    Toml(toml::Value),
    Json(serde_json::Value),
}

#[derive(Debug)]
pub struct ConfigApplyReport {
    pub target: String,
    pub backup: Option<PathBuf>,
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Allowlist {
    #[serde(default)]
    allowed_targets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConfigLock {
    #[serde(default, rename = "target")]
    targets: Vec<ConfigTargetLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigTargetLock {
    path: String,
    format: PatchFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    sha512: String,
}

#[derive(Debug, Deserialize)]
struct TomlPatchFile {
    target: String,
    format: String,
    #[serde(default)]
    merge: Option<String>,
    #[serde(default)]
    package: Option<String>,
    data: toml::Value,
}

#[derive(Debug, Deserialize)]
struct JsonPatchFile {
    target: String,
    format: String,
    #[serde(default)]
    merge: Option<String>,
    #[serde(default)]
    package: Option<String>,
    data: serde_json::Value,
}

impl ConfigPatch {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read config patch {}", path.display()))?;
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("toml") => {
                let raw: TomlPatchFile = toml::from_str(&text)
                    .with_context(|| format!("failed to parse TOML patch {}", path.display()))?;
                let format = PatchFormat::parse(&raw.format)?;
                if format != PatchFormat::Toml {
                    bail!("TOML patch {} must target format toml", path.display());
                }
                Ok(Self {
                    target: raw.target,
                    format,
                    package: raw.package,
                    merge: parse_merge(raw.merge.as_deref())?,
                    data: PatchData::Toml(raw.data),
                })
            }
            Some("json") => {
                let raw: JsonPatchFile = serde_json::from_str(&text)
                    .with_context(|| format!("failed to parse JSON patch {}", path.display()))?;
                let format = PatchFormat::parse(&raw.format)?;
                if format != PatchFormat::Json {
                    bail!("JSON patch {} must target format json", path.display());
                }
                Ok(Self {
                    target: raw.target,
                    format,
                    package: raw.package,
                    merge: parse_merge(raw.merge.as_deref())?,
                    data: PatchData::Json(raw.data),
                })
            }
            _ => bail!(
                "config patch {} must have a .toml or .json extension",
                path.display()
            ),
        }
    }
}

pub fn allow_target(instance: &InstancePaths, target: &str) -> Result<()> {
    validate_target(target)?;
    instance.ensure_state_dirs()?;
    let path = instance.state_dir().join(ALLOWLIST_FILE);
    let mut allowlist = if path.is_file() {
        load_allowlist(&path)?
    } else {
        Allowlist::default()
    };
    if !allowlist.allowed_targets.iter().any(|item| item == target) {
        allowlist.allowed_targets.push(target.to_owned());
        allowlist.allowed_targets.sort();
        write_atomic(&path, toml::to_string_pretty(&allowlist)?.as_bytes())?;
    }
    Ok(())
}

pub fn apply_patch(instance: &InstancePaths, patch: &ConfigPatch) -> Result<ConfigApplyReport> {
    validate_target(&patch.target)?;
    let allowlist = load_allowlist(&instance.state_dir().join(ALLOWLIST_FILE))?;
    if !allowlist
        .allowed_targets
        .iter()
        .any(|target| target == &patch.target)
    {
        bail!(
            "config target {} is not allowed; run `dpp config allow {}`",
            patch.target,
            patch.target
        );
    }

    let target_path = instance.root().join(&patch.target);
    let target_exists = target_path.is_file();
    let current_bytes =
        if target_exists {
            Some(fs::read(&target_path).with_context(|| {
                format!("failed to read config target {}", target_path.display())
            })?)
        } else {
            None
        };
    let current_hash = current_bytes.as_deref().map(sha512_bytes);
    let lock_path = instance.state_dir().join(LOCK_FILE);
    let mut lock = load_lock(&lock_path)?;
    let baseline = lock
        .targets
        .iter()
        .find(|target| target.path == patch.target)
        .cloned();

    if let Some(baseline) = &baseline {
        if current_hash.as_deref() != Some(baseline.sha512.as_str()) {
            let patched = render_patch(patch, current_bytes.as_deref())?;
            let conflict_path = target_path.with_extension(format!(
                "{}dppnew",
                target_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map_or_else(String::new, |extension| format!("{extension}."))
            ));
            write_atomic(&conflict_path, patched.as_bytes())?;
            bail!(
                "config target {} was modified after dpp applied it; wrote {}",
                patch.target,
                conflict_path.display()
            );
        }
    }

    let rendered = render_patch(patch, current_bytes.as_deref())?;
    let new_hash = sha512_bytes(rendered.as_bytes());
    let backup = backup_target(instance, &patch.target, current_bytes.as_deref())?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&target_path, rendered.as_bytes())?;

    lock.targets.retain(|target| target.path != patch.target);
    lock.targets.push(ConfigTargetLock {
        path: patch.target.clone(),
        format: patch.format,
        package: patch.package.clone(),
        sha512: new_hash.clone(),
    });
    lock.targets
        .sort_by(|left, right| left.path.cmp(&right.path));
    if let Err(error) = write_atomic(&lock_path, toml::to_string_pretty(&lock)?.as_bytes()) {
        if let Some(bytes) = current_bytes {
            let _ = write_atomic(&target_path, &bytes);
        } else {
            let _ = fs::remove_file(&target_path);
        }
        return Err(error);
    }

    Ok(ConfigApplyReport {
        target: patch.target.clone(),
        backup,
        hash: new_hash,
    })
}

fn render_patch(patch: &ConfigPatch, current: Option<&[u8]>) -> Result<String> {
    match (&patch.data, patch.format) {
        (PatchData::Toml(patched), PatchFormat::Toml) => {
            let base = match current {
                Some(bytes) => toml::from_str::<toml::Value>(
                    std::str::from_utf8(bytes).context("current config is not valid UTF-8")?,
                )
                .context("current config is not valid TOML")?,
                None => toml::Value::Table(toml::map::Map::new()),
            };
            let value = match patch.merge {
                MergeMode::Replace => patched.clone(),
                MergeMode::Merge => merge_toml(base, patched.clone()),
            };
            Ok(toml::to_string_pretty(&value)?)
        }
        (PatchData::Json(patched), PatchFormat::Json) => {
            let base = match current {
                Some(bytes) => serde_json::from_slice::<serde_json::Value>(bytes)
                    .context("current config is not valid JSON")?,
                None => serde_json::Value::Object(serde_json::Map::new()),
            };
            let value = match patch.merge {
                MergeMode::Replace => patched.clone(),
                MergeMode::Merge => merge_json(base, patched.clone()),
            };
            Ok(serde_json::to_string_pretty(&value)?)
        }
        _ => bail!("patch data format does not match target format"),
    }
}

fn merge_toml(mut base: toml::Value, patch: toml::Value) -> toml::Value {
    match (&mut base, patch) {
        (toml::Value::Table(base), toml::Value::Table(patch)) => {
            for (key, value) in patch {
                let merged = match base.remove(&key) {
                    Some(existing) => merge_toml(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            toml::Value::Table(base.clone())
        }
        (_, patch) => patch,
    }
}

fn merge_json(mut base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (&mut base, patch) {
        (serde_json::Value::Object(base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                let merged = match base.remove(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            serde_json::Value::Object(base.clone())
        }
        (_, patch) => patch,
    }
}

fn parse_merge(value: Option<&str>) -> Result<MergeMode> {
    match value.unwrap_or("merge") {
        "merge" => Ok(MergeMode::Merge),
        "replace" => Ok(MergeMode::Replace),
        other => bail!("unsupported merge mode {other:?}; use merge or replace"),
    }
}

fn validate_target(target: &str) -> Result<()> {
    let path = Path::new(target);
    if target.is_empty()
        || target.len() > 240
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("invalid config target {target:?}");
    }
    Ok(())
}

fn load_allowlist(path: &Path) -> Result<Allowlist> {
    if !path.is_file() {
        bail!(
            "config allowlist is missing: {}; run `dpp config allow <target>`",
            path.display()
        );
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn load_lock(path: &Path) -> Result<ConfigLock> {
    if !path.is_file() {
        return Ok(ConfigLock::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn backup_target(
    instance: &InstancePaths,
    target: &str,
    current: Option<&[u8]>,
) -> Result<Option<PathBuf>> {
    let Some(current) = current else {
        return Ok(None);
    };
    let backup = instance
        .state_dir()
        .join(BACKUP_DIR)
        .join(timestamp_nanos().to_string())
        .join(target);
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(&backup, current)?;
    Ok(Some(backup))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

/// Used by `config status`; not needed by the patch merge engine itself.
pub fn target_lock_summary(instance: &InstancePaths) -> Result<Vec<(String, String)>> {
    let lock = load_lock(&instance.state_dir().join(LOCK_FILE))?;
    Ok(lock
        .targets
        .into_iter()
        .map(|target| (target.path, target.sha512))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance() -> (tempfile::TempDir, InstancePaths) {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("server.jar"), b"jar").unwrap();
        let instance = InstancePaths::discover(Some(directory.path())).unwrap();
        instance.ensure_state_dirs().unwrap();
        (directory, instance)
    }

    #[test]
    fn toml_patch_merges_and_detects_user_changes() {
        let (directory, instance) = instance();
        fs::create_dir_all(directory.path().join("config")).unwrap();
        fs::write(
            directory.path().join("config/app.toml"),
            "a = 1\n[nested]\nx = 1\n",
        )
        .unwrap();
        let patch_path = directory.path().join("patch.toml");
        fs::write(
            &patch_path,
            r#"
target = "config/app.toml"
format = "toml"
merge = "merge"

[data]
b = 2

[data.nested]
y = 2
"#,
        )
        .unwrap();
        allow_target(&instance, "config/app.toml").unwrap();
        let patch = ConfigPatch::load(&patch_path).unwrap();
        let report = apply_patch(&instance, &patch).unwrap();
        assert!(report.backup.as_ref().is_some_and(|path| path.is_file()));
        let value = fs::read_to_string(directory.path().join("config/app.toml")).unwrap();
        assert!(value.contains("a = 1"));
        assert!(value.contains("b = 2"));
        assert!(value.contains("y = 2"));

        fs::write(directory.path().join("config/app.toml"), "user = true\n").unwrap();
        let error = apply_patch(&instance, &patch).unwrap_err();
        assert!(error.to_string().contains("was modified"));
        assert!(directory.path().join("config/app.toml.dppnew").is_file());
    }

    #[test]
    fn json_patch_merges_existing_object() {
        let (directory, instance) = instance();
        fs::create_dir_all(directory.path().join("config")).unwrap();
        fs::write(
            directory.path().join("config/app.json"),
            r#"{"a":1,"nested":{"x":1}}"#,
        )
        .unwrap();
        let patch_path = directory.path().join("patch.json");
        fs::write(
            &patch_path,
            r#"{"target":"config/app.json","format":"json","merge":"merge","data":{"b":2,"nested":{"y":2}}}"#,
        )
        .unwrap();
        allow_target(&instance, "config/app.json").unwrap();
        let patch = ConfigPatch::load(&patch_path).unwrap();
        apply_patch(&instance, &patch).unwrap();
        let value: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(directory.path().join("config/app.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["a"], 1);
        assert_eq!(value["b"], 2);
        assert_eq!(value["nested"]["y"], 2);
    }
}
