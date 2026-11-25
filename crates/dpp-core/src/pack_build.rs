use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::copy;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use tempfile::NamedTempFile;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::error::{DppError, DppResult};
use crate::hash::sha512_file;
use crate::manifest::DEFAULT_LOAD_WEIGHT;
use crate::pack_manifest::PackManifest;
use crate::pack_meta::{PACK_META_FILE, PackMeta};

const MAX_META_BYTES: u64 = 1024 * 1024;
#[derive(Debug, Clone)]
pub struct BuildPackOptions {
    pub source: PathBuf,
    pub output: Option<PathBuf>,
    pub url: Option<String>,
}

impl BuildPackOptions {
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            output: None,
            url: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuiltPack {
    pub path: PathBuf,
    pub sha512: String,
    pub size: u64,
    pub files: usize,
    pub manifest: PackManifest,
    pub index_entry: Value,
}

/// Build a deterministic datapack zip from a source directory.
///
/// The source root must contain `pack.mcmeta` and `dpp.toml`. File order,
/// timestamps, permissions, and compression are fixed so repeated builds of
/// unchanged inputs produce the same sha512.
pub fn build_pack(options: BuildPackOptions) -> DppResult<BuiltPack> {
    let source = options.source;
    if !source.is_dir() {
        return Err(DppError::PackBuildSource(source));
    }
    let (pack_root, meta_path, manifest_path, inject_manifest) = resolve_pack_root(&source)?;
    let source_abs = fs::canonicalize(&pack_root).map_err(|error| DppError::Read {
        path: pack_root.clone(),
        source: error,
    })?;

    let meta_text = read_small_text(&meta_path, MAX_META_BYTES)?;
    let meta = PackMeta::from_json_str(&meta_text).map_err(|source| DppError::PackMetaJson {
        path: meta_path.clone(),
        source,
    })?;
    let raw_meta: Value =
        serde_json::from_str(&meta_text).map_err(|source| DppError::PackMetaJson {
            path: meta_path.clone(),
            source,
        })?;

    let manifest_text = read_small_text(&manifest_path, MAX_META_BYTES)?;
    let manifest = PackManifest::from_toml_str(&manifest_text, &manifest_path)?;
    for format in [
        manifest.package.pack_format,
        manifest.package.min_format,
        manifest.package.max_format,
    ]
    .into_iter()
    .flatten()
    {
        if !meta.supports(format) {
            return Err(DppError::PackBuildInvalid {
                path: pack_root.clone(),
                reason: format!(
                    "dpp.toml targets pack format {format}, but pack.mcmeta does not support it"
                ),
            });
        }
    }

    let output = options
        .output
        .unwrap_or_else(|| default_output(&source_abs, &manifest));
    if output.file_name().is_none() {
        return Err(DppError::PackBuildInvalid {
            path: output,
            reason: "output must name a zip file".to_owned(),
        });
    }
    if output
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("jar"))
    {
        return Err(DppError::PackBuildInvalid {
            path: output,
            reason: "build-pack output must not be a .jar".to_owned(),
        });
    }
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| DppError::CreateDir {
        path: parent.to_path_buf(),
        source,
    })?;
    let parent_abs = fs::canonicalize(parent).map_err(|error| DppError::Read {
        path: parent.to_path_buf(),
        source: error,
    })?;
    let output_abs = parent_abs.join(output.file_name().unwrap());
    if output_abs.starts_with(&source_abs) {
        return Err(DppError::PackBuildOutputInsideSource(output));
    }

    let mut files = BTreeMap::new();
    let mut folded_paths = BTreeSet::new();
    collect_files(&source_abs, &source_abs, &mut files, &mut folded_paths)?;
    if inject_manifest
        && files
            .keys()
            .any(|name| name.eq_ignore_ascii_case("dpp.toml"))
    {
        return Err(DppError::PackBuildInvalid {
            path: manifest_path,
            reason: "src/ already contains dpp.toml; refusing to inject a second one".to_owned(),
        });
    }
    if inject_manifest {
        files.insert("dpp.toml".to_owned(), manifest_path.clone());
    }
    if !files.contains_key(PACK_META_FILE) {
        return Err(DppError::PackMetaMissing(meta_path));
    }
    if !files.contains_key("dpp.toml") {
        return Err(DppError::PackBuildInvalid {
            path: manifest_path,
            reason: "dpp.toml is required for build-pack".to_owned(),
        });
    }

    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| DppError::Write {
        path: output.clone(),
        source,
    })?;
    {
        let mut writer = ZipWriter::new(temporary.as_file_mut());
        let zip_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (name, path) in &files {
            writer
                .start_file(name, zip_options)
                .map_err(|source| DppError::PackBuildInvalid {
                    path: output.clone(),
                    reason: format!("failed to add {name}: {source}"),
                })?;
            let mut input = File::open(path).map_err(|source| DppError::Read {
                path: path.clone(),
                source,
            })?;
            copy(&mut input, &mut writer).map_err(|source| DppError::Write {
                path: output.clone(),
                source,
            })?;
        }
        writer
            .finish()
            .map_err(|source| DppError::PackBuildInvalid {
                path: output.clone(),
                reason: format!("failed to finish zip: {source}"),
            })?;
    }
    temporary
        .persist(&output)
        .map_err(|error| DppError::Write {
            path: output.clone(),
            source: error.error,
        })?;

    let target = manifest.package.pack_format.or(manifest.package.min_format);
    if let Some(target) = target {
        crate::pack_archive::inspect_zip(&output, target)?;
    }

    let sha512 = sha512_file(&output)?;
    let size = fs::metadata(&output)
        .map_err(|source| DppError::Read {
            path: output.clone(),
            source,
        })?
        .len();
    let url = options
        .url
        .unwrap_or_else(|| output.file_name().unwrap().to_string_lossy().into_owned());
    let index_entry = build_index_entry(&manifest, &raw_meta, &url, &sha512, size);

    Ok(BuiltPack {
        path: output,
        sha512,
        size,
        files: files.len(),
        manifest,
        index_entry,
    })
}

fn resolve_pack_root(source: &Path) -> DppResult<(PathBuf, PathBuf, PathBuf, bool)> {
    let root_meta = source.join(PACK_META_FILE);
    if root_meta.is_file() {
        return Ok((
            source.to_path_buf(),
            root_meta,
            source.join("dpp.toml"),
            false,
        ));
    }

    let nested_root = source.join("src");
    let nested_meta = nested_root.join(PACK_META_FILE);
    if nested_meta.is_file() {
        let nested_manifest = nested_root.join("dpp.toml");
        let manifest = if nested_manifest.is_file() {
            nested_manifest
        } else {
            source.join("dpp.toml")
        };
        let inject_manifest = !source.join("src/dpp.toml").is_file();
        return Ok((nested_root, nested_meta, manifest, inject_manifest));
    }

    Err(DppError::PackMetaMissing(root_meta))
}

fn default_output(source: &Path, manifest: &PackManifest) -> PathBuf {
    let id = manifest.package.id.replace(':', "_");
    let name = format!("{id}-{}.zip", manifest.package.version);
    source
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

fn read_small_text(path: &Path, limit: u64) -> DppResult<String> {
    let size = fs::metadata(path).map_err(|source| DppError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if size.len() > limit {
        return Err(DppError::PackBuildInvalid {
            path: path.to_path_buf(),
            reason: format!("file is larger than {limit} bytes"),
        });
    }
    fs::read_to_string(path).map_err(|source| DppError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn zip_path_name(root: &Path, path: &Path) -> DppResult<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| DppError::PackBuildInvalid {
            path: path.to_path_buf(),
            reason: "path is outside the source root".to_owned(),
        })?;
    let entry = relative.to_string_lossy().replace('\\', "/");
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(DppError::PackUnsafeEntry {
                path: root.to_path_buf(),
                entry,
            });
        };
        let Some(part) = part.to_str() else {
            return Err(DppError::PackBuildInvalid {
                path: path.to_path_buf(),
                reason: "non-UTF-8 paths are not supported".to_owned(),
            });
        };
        if part.contains('/') || part.contains('\\') {
            return Err(DppError::PackUnsafeEntry {
                path: root.to_path_buf(),
                entry,
            });
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(DppError::PackUnsafeEntry {
            path: root.to_path_buf(),
            entry,
        });
    }
    Ok(parts.join("/"))
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, PathBuf>,
    folded_paths: &mut BTreeSet<String>,
) -> DppResult<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|source| DppError::Read {
            path: current.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| DppError::Read {
            path: current.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| DppError::Read {
            path: path.clone(),
            source,
        })?;
        let name = zip_path_name(root, &path)?;
        if metadata.file_type().is_symlink() {
            return Err(DppError::PackUnsafeEntry {
                path: root.to_path_buf(),
                entry: name,
            });
        }
        if metadata.is_dir() {
            collect_files(root, &path, files, folded_paths)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(DppError::PackUnsafeEntry {
                path: root.to_path_buf(),
                entry: name,
            });
        }
        let folded = name.to_ascii_lowercase();
        if !folded_paths.insert(folded) {
            return Err(DppError::PackBuildInvalid {
                path: root.to_path_buf(),
                reason: format!("case-insensitive duplicate path: {name}"),
            });
        }
        files.insert(name, path);
    }
    Ok(())
}

fn build_index_entry(
    manifest: &PackManifest,
    raw_meta: &Value,
    url: &str,
    sha512: &str,
    size: u64,
) -> Value {
    let pack = raw_meta.get("pack").cloned().unwrap_or(Value::Null);
    let mut entry = json!({
        "id": manifest.package.id,
        "version": manifest.package.version,
        "url": url,
        "sha512": sha512,
        "size": size,
        "mc_versions": manifest.package.mc_versions,
        "dependencies": manifest.dependencies,
        "conflicts": manifest.conflicts,
        "host": manifest.host,
        "load_weight": DEFAULT_LOAD_WEIGHT,
        "yanked": false,
    });
    let object = entry.as_object_mut().expect("json object");
    for key in [
        "min_format",
        "max_format",
        "pack_format",
        "supported_formats",
    ] {
        if let Some(value) = pack.get(key) {
            object.insert(key.to_owned(), value.clone());
        }
    }
    entry
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use zip::ZipArchive;

    use super::*;
    use crate::pack_archive::inspect_zip;
    use crate::pack_format::PackFormat;

    fn source_pack() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("pack.mcmeta"),
            r#"{"pack":{"description":"x","min_format":[121,0],"max_format":[121,0]}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("dpp.toml"),
            "[package]\nid = \"local:demo\"\nversion = \"0.1.0\"\npack_format = [121, 0]\n",
        )
        .unwrap();
        fs::create_dir_all(directory.path().join("data/demo/function")).unwrap();
        fs::write(
            directory.path().join("data/demo/function/load.mcfunction"),
            "say hi\n",
        )
        .unwrap();
        directory
    }

    #[test]
    fn builds_deterministic_pack() {
        let source = source_pack();
        let first = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: Some(source.path().parent().unwrap().join("first.zip")),
            url: None,
        })
        .unwrap();
        let second = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: Some(source.path().parent().unwrap().join("second.zip")),
            url: None,
        })
        .unwrap();

        assert_eq!(first.sha512, second.sha512);
        assert_eq!(first.index_entry["id"], "local:demo");
        assert!(first.index_entry["sha512"].as_str().unwrap() == first.sha512);
        inspect_zip(&first.path, PackFormat::new(121, 0)).unwrap();

        let file = File::open(&first.path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            names.push(archive.by_index(index).unwrap().name().to_owned());
        }
        assert_eq!(
            names,
            [
                "data/demo/function/load.mcfunction",
                "dpp.toml",
                "pack.mcmeta"
            ]
        );
        let mut text = String::new();
        archive
            .by_name("pack.mcmeta")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("121"));
    }

    #[test]
    fn rejects_manifest_format_outside_pack_meta() {
        let source = source_pack();
        fs::write(
            source.path().join("pack.mcmeta"),
            r#"{"pack":{"description":"x","min_format":[120,0],"max_format":[120,0]}}"#,
        )
        .unwrap();
        let error = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: Some(source.path().parent().unwrap().join("bad.zip")),
            url: None,
        })
        .unwrap_err();
        assert!(matches!(error, DppError::PackBuildInvalid { .. }));
    }

    #[test]
    fn rejects_output_inside_source() {
        let source = source_pack();
        let error = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: Some(source.path().join("out.zip")),
            url: None,
        })
        .unwrap_err();
        assert!(matches!(error, DppError::PackBuildOutputInsideSource(_)));
    }
    #[test]
    fn builds_project_root_with_src_tree() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("src");
        fs::create_dir_all(source.join("data/demo/function")).unwrap();
        fs::write(
            project.path().join("dpp.toml"),
            "[package]\nid = \"local:demo\"\nversion = \"0.1.0\"\npack_format = [121, 0]\n",
        )
        .unwrap();
        fs::write(
            source.join("pack.mcmeta"),
            r#"{"pack":{"description":"x","min_format":[121,0],"max_format":[121,0]}}"#,
        )
        .unwrap();
        fs::write(
            source.join("data/demo/function/load.mcfunction"),
            "say hi\n",
        )
        .unwrap();

        let built = build_pack(BuildPackOptions {
            source: project.path().to_path_buf(),
            output: Some(project.path().join("demo.zip")),
            url: None,
        })
        .unwrap();
        assert_eq!(built.index_entry["id"], "local:demo");

        let file = File::open(&built.path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"dpp.toml".to_owned()));
        assert!(names.contains(&"pack.mcmeta".to_owned()));
    }
    #[test]
    fn rejects_manifest_range_wider_than_pack_meta() {
        let source = source_pack();
        fs::write(
            source.path().join("dpp.toml"),
            "[package]\nid = \"local:demo\"\nversion = \"0.1.0\"\nmin_format = [120, 0]\nmax_format = [122, 0]\n",
        )
        .unwrap();
        let error = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: Some(source.path().parent().unwrap().join("range.zip")),
            url: None,
        })
        .unwrap_err();
        assert!(matches!(error, DppError::PackBuildInvalid { .. }));
    }
    #[test]
    fn default_output_is_outside_source() {
        let source = source_pack();
        let built = build_pack(BuildPackOptions {
            source: source.path().to_path_buf(),
            output: None,
            url: None,
        })
        .unwrap();
        assert!(!built.path.starts_with(source.path()));
        let _ = fs::remove_file(&built.path);
    }
}
