use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha512};
use zip::ZipArchive;

use crate::error::{DppError, DppResult};
use crate::hash::to_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    pub path: String,
    pub sha512: String,
}

/// Extract a zip into `destination`, rejecting unsafe entries and symlinks.
pub fn extract_zip(zip_path: &Path, destination: &Path) -> DppResult<Vec<ExtractedFile>> {
    let file = File::open(zip_path).map_err(|source| DppError::Read {
        path: zip_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| DppError::PackZip {
        path: zip_path.to_path_buf(),
        source,
    })?;

    fs::create_dir_all(destination).map_err(|source| DppError::CreateDir {
        path: destination.to_path_buf(),
        source,
    })?;

    let mut extracted = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|source| DppError::PackZip {
                path: zip_path.to_path_buf(),
                source,
            })?;
        let name = entry.name().to_owned();

        if name.contains('\\') {
            return Err(DppError::PackUnsafeEntry {
                path: zip_path.to_path_buf(),
                entry: name,
            });
        }
        let relative = match entry.enclosed_name() {
            Some(path) => path.to_path_buf(),
            None => {
                return Err(DppError::PackUnsafeEntry {
                    path: zip_path.to_path_buf(),
                    entry: name,
                });
            }
        };

        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(DppError::PackUnsafeEntry {
                    path: zip_path.to_path_buf(),
                    entry: name,
                });
            }
        }

        let target = destination.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|source| DppError::CreateDir {
                path: target,
                source,
            })?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| DppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let mut output = File::create(&target).map_err(|source| DppError::Write {
            path: target.clone(),
            source,
        })?;
        let mut hasher = Sha512::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer).map_err(|source| DppError::Read {
                path: zip_path.to_path_buf(),
                source,
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            output
                .write_all(&buffer[..read])
                .map_err(|source| DppError::Write {
                    path: target.clone(),
                    source,
                })?;
        }
        output.sync_all().map_err(|source| DppError::Write {
            path: target.clone(),
            source,
        })?;

        extracted.push(ExtractedFile {
            path: normalized_path(&relative),
            sha512: to_hex(hasher.finalize().as_slice()),
        });
    }

    Ok(extracted)
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        for (name, contents) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn extracts_files_and_hashes_them() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");
        write_zip(
            &archive,
            &[
                ("pack.mcmeta", br#"{"pack":{"description":"x"}}"#),
                ("data/example/function/hello.mcfunction", b"say hello"),
            ],
        );
        let destination = directory.path().join("out");

        let files = extract_zip(&archive, &destination).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(
            fs::read(destination.join("data/example/function/hello.mcfunction")).unwrap(),
            b"say hello"
        );
        assert!(files.iter().any(|file| file.path == "pack.mcmeta"));
    }

    #[test]
    fn rejects_unsafe_entries() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("pack.zip");

        let file = File::create(&archive).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file(
                "../evil",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(b"evil").unwrap();
        writer.finish().unwrap();

        assert!(extract_zip(&archive, &directory.path().join("out")).is_err());
    }
}
