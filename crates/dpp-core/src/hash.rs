use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha512};

use crate::error::{DppError, DppResult};

pub fn sha512_file(path: &Path) -> DppResult<String> {
    let mut file = File::open(path).map_err(|source| DppError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha512::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer).map_err(|source| DppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(to_hex(hasher.finalize().as_slice()))
}

pub fn sha512_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    to_hex(hasher.finalize().as_slice())
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_bytes_and_files_consistently() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("payload");
        std::fs::write(&path, b"hello").unwrap();

        assert_eq!(sha512_file(&path).unwrap(), sha512_bytes(b"hello"));
    }
}
