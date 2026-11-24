use std::fs::File;
use std::io::Read;
use std::path::Path;

use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::{DppError, DppResult};
use crate::pack_format::PackFormat;
use crate::pack_meta::PackMeta;

const MAX_PACK_META_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PackInspection {
    pub meta: PackMeta,
    pub file_count: usize,
    pub total_uncompressed_size: u64,
}

/// Inspect a data pack zip before installing it.
///
/// Rejects nested `pack.mcmeta`, unsafe paths, symlinks, and incompatible
/// data pack formats.
pub fn inspect_zip(path: &Path, target: PackFormat) -> DppResult<PackInspection> {
    if has_jar_extension(path) {
        return Err(DppError::PackNotZip(path.to_path_buf()));
    }

    let file = File::open(path).map_err(|source| DppError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| DppError::PackZip {
        path: path.to_path_buf(),
        source,
    })?;

    let meta_text = read_pack_meta(&mut archive, path)?;
    let meta = PackMeta::from_json_str(&meta_text).map_err(|source| DppError::PackMetaJson {
        path: path.to_path_buf(),
        source,
    })?;

    if !meta.supports(target) {
        return Err(DppError::PackIncompatible {
            path: path.to_path_buf(),
            target,
        });
    }

    let (file_count, total_uncompressed_size) = scan_entries(&mut archive, path)?;
    Ok(PackInspection {
        meta,
        file_count,
        total_uncompressed_size,
    })
}

fn read_pack_meta(archive: &mut ZipArchive<File>, path: &Path) -> DppResult<String> {
    let text = match archive.by_name("pack.mcmeta") {
        Ok(entry) => {
            if entry.is_dir() {
                return Err(DppError::PackMetaMissing(path.to_path_buf()));
            }
            if entry.size() > MAX_PACK_META_BYTES {
                return Err(DppError::PackMetaTooLarge {
                    path: path.to_path_buf(),
                    size: entry.size(),
                });
            }

            let mut text = String::new();
            entry
                .take(MAX_PACK_META_BYTES + 1)
                .read_to_string(&mut text)
                .map_err(|source| DppError::Read {
                    path: path.to_path_buf(),
                    source,
                })?;
            if text.len() as u64 > MAX_PACK_META_BYTES {
                return Err(DppError::PackMetaTooLarge {
                    path: path.to_path_buf(),
                    size: text.len() as u64,
                });
            }
            Some(text)
        }
        Err(ZipError::FileNotFound) => None,
        Err(source) => {
            return Err(DppError::PackZip {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if let Some(text) = text {
        return Ok(text);
    }

    if let Some(entry) = find_nested_meta(archive, path)? {
        return Err(DppError::PackMetaNested {
            path: path.to_path_buf(),
            entry,
        });
    }
    Err(DppError::PackMetaMissing(path.to_path_buf()))
}

fn find_nested_meta(archive: &mut ZipArchive<File>, path: &Path) -> DppResult<Option<String>> {
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|source| DppError::PackZip {
                path: path.to_path_buf(),
                source,
            })?;
        let name = entry.name().to_owned();
        if name.ends_with("/pack.mcmeta") {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

fn scan_entries(archive: &mut ZipArchive<File>, path: &Path) -> DppResult<(usize, u64)> {
    let mut file_count = 0usize;
    let mut total_uncompressed_size = 0u64;

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|source| DppError::PackZip {
                path: path.to_path_buf(),
                source,
            })?;
        let name = entry.name().to_owned();

        if name.contains('\\') || entry.enclosed_name().is_none() {
            return Err(DppError::PackUnsafeEntry {
                path: path.to_path_buf(),
                entry: name,
            });
        }

        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(DppError::PackUnsafeEntry {
                    path: path.to_path_buf(),
                    entry: name,
                });
            }
        }

        if !entry.is_dir() {
            file_count += 1;
            total_uncompressed_size = total_uncompressed_size.saturating_add(entry.size());
        }
    }

    Ok((file_count, total_uncompressed_size))
}

fn has_jar_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    const META_121: &str = r#"{
        "pack": {
            "description": "test",
            "min_format": [121, 0],
            "max_format": [121, 0]
        }
    }"#;

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        for (name, contents) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn accepts_valid_root_pack_meta() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        write_zip(
            &archive,
            &[
                ("pack.mcmeta", META_121),
                ("data/example/function/hello.mcfunction", "say hello"),
            ],
        );

        let inspection = inspect_zip(&archive, PackFormat::new(121, 0)).unwrap();
        assert!(inspection.meta.supports(PackFormat::new(121, 0)));
        assert_eq!(inspection.file_count, 2);
        assert!(inspection.total_uncompressed_size > 0);
    }

    #[test]
    fn rejects_incompatible_pack() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        write_zip(&archive, &[("pack.mcmeta", META_121)]);

        assert!(matches!(
            inspect_zip(&archive, PackFormat::new(88, 0)),
            Err(DppError::PackIncompatible { .. })
        ));
    }

    #[test]
    fn rejects_nested_pack_meta() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        write_zip(&archive, &[("nested/pack.mcmeta", META_121)]);

        assert!(matches!(
            inspect_zip(&archive, PackFormat::new(121, 0)),
            Err(DppError::PackMetaNested { .. })
        ));
    }

    #[test]
    fn rejects_missing_pack_meta() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        write_zip(
            &archive,
            &[("data/example/function/hello.mcfunction", "say hi")],
        );

        assert!(matches!(
            inspect_zip(&archive, PackFormat::new(121, 0)),
            Err(DppError::PackMetaMissing(_))
        ));
    }

    #[test]
    fn rejects_jar_extension() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.jar");
        write_zip(&archive, &[("pack.mcmeta", META_121)]);

        assert!(matches!(
            inspect_zip(&archive, PackFormat::new(121, 0)),
            Err(DppError::PackNotZip(_))
        ));
    }

    #[test]
    fn rejects_unsafe_paths() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");

        write_zip(
            &archive,
            &[("pack.mcmeta", META_121), ("../evil.txt", "evil")],
        );

        assert!(matches!(
            inspect_zip(&archive, PackFormat::new(121, 0)),
            Err(DppError::PackUnsafeEntry { .. })
        ));
    }
}
