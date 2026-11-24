use std::fs;
use std::io::{Read, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha512};
use tempfile::NamedTempFile;

use crate::types::{DownloadReport, ProjectVersion, SearchResponse, VersionFile};

const DEFAULT_BASE_URL: &str = "https://api.modrinth.com/v2";

#[derive(Debug, thiserror::Error)]
pub enum ModrinthError {
    #[error("User-Agent must not be empty")]
    EmptyUserAgent,

    #[error("invalid User-Agent: {0}")]
    InvalidUserAgent(String),

    #[error("failed to build HTTP client: {source}")]
    HttpClient {
        #[source]
        source: reqwest::Error,
    },

    #[error("request to {url} failed: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to read download from {url}: {source}")]
    DownloadRead {
        url: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Modrinth returned HTTP {status} for {url}: {body}")]
    HttpStatus {
        url: String,
        status: u16,
        body: String,
    },

    #[error("failed to serialize request JSON: {0}")]
    Serialize(serde_json::Error),

    #[error("failed to decode Modrinth response from {url}: {source}")]
    Decode {
        url: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to create download directory {path}: {source}")]
    DownloadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write downloaded artifact {path}: {source}")]
    DownloadWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("downloaded artifact size mismatch for {url}: expected {expected}, got {actual}")]
    SizeMismatch {
        url: String,
        expected: u64,
        actual: u64,
    },

    #[error("downloaded artifact sha512 mismatch for {url}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },
}

#[derive(Debug, Clone)]
pub struct ModrinthClient {
    http: Client,
    base_url: String,
}

impl ModrinthClient {
    pub fn new(user_agent: impl AsRef<str>) -> Result<Self, ModrinthError> {
        Self::with_base_url(DEFAULT_BASE_URL, user_agent)
    }

    pub fn with_base_url(
        base_url: impl Into<String>,
        user_agent: impl AsRef<str>,
    ) -> Result<Self, ModrinthError> {
        let user_agent = user_agent.as_ref();
        if user_agent.trim().is_empty() {
            return Err(ModrinthError::EmptyUserAgent);
        }

        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(user_agent)
            .map_err(|_| ModrinthError::InvalidUserAgent(user_agent.to_owned()))?;
        headers.insert(USER_AGENT, value);

        let http = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|source| ModrinthError::HttpClient { source })?;

        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        })
    }

    pub fn search(
        &self,
        query: &str,
        facets: &[Vec<String>],
        limit: u8,
        offset: u32,
    ) -> Result<SearchResponse, ModrinthError> {
        let facets = serde_json::to_string(facets).map_err(ModrinthError::Serialize)?;
        let query = [
            ("query", query.to_owned()),
            ("facets", facets),
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
        ];
        self.get_json("/search", &query)
    }

    pub fn search_datapacks(
        &self,
        query: &str,
        mc_version: Option<&str>,
        limit: u8,
    ) -> Result<SearchResponse, ModrinthError> {
        let mut facets = vec![vec!["categories:datapack".to_owned()]];
        if let Some(version) = mc_version {
            facets.push(vec![format!("versions:{version}")]);
        }
        self.search(query, &facets, limit, 0)
    }

    pub fn project_versions(
        &self,
        project_id: &str,
        game_version: Option<&str>,
    ) -> Result<Vec<ProjectVersion>, ModrinthError> {
        let mut query = vec![
            ("loaders", "[\"datapack\"]".to_owned()),
            ("include_changelog", "false".to_owned()),
        ];
        if let Some(version) = game_version {
            let versions = serde_json::to_string(&[version]).map_err(ModrinthError::Serialize)?;
            query.push(("game_versions", versions));
        }

        self.get_json(&format!("/project/{project_id}/version"), &query)
    }

    /// Download a version file atomically, verifying its size and sha512.
    pub fn download_file(
        &self,
        file: &VersionFile,
        destination: &Path,
    ) -> Result<DownloadReport, ModrinthError> {
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| ModrinthError::DownloadDir {
            path: parent.to_path_buf(),
            source,
        })?;

        let mut response =
            self.http
                .get(&file.url)
                .send()
                .map_err(|source| ModrinthError::Request {
                    url: file.url.clone(),
                    source,
                })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().map_err(|source| ModrinthError::Request {
                url: file.url.clone(),
                source,
            })?;
            return Err(ModrinthError::HttpStatus {
                url: file.url.clone(),
                status: status.as_u16(),
                body,
            });
        }

        let mut temporary =
            NamedTempFile::new_in(parent).map_err(|source| ModrinthError::DownloadWrite {
                path: destination.to_path_buf(),
                source,
            })?;
        let mut hasher = Sha512::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 64 * 1024];

        loop {
            let read =
                response
                    .read(&mut buffer)
                    .map_err(|source| ModrinthError::DownloadRead {
                        url: file.url.clone(),
                        source,
                    })?;
            if read == 0 {
                break;
            }

            size = size.saturating_add(read as u64);
            if file.size != 0 && size > file.size {
                return Err(ModrinthError::SizeMismatch {
                    url: file.url.clone(),
                    expected: file.size,
                    actual: size,
                });
            }

            hasher.update(&buffer[..read]);
            temporary.write_all(&buffer[..read]).map_err(|source| {
                ModrinthError::DownloadWrite {
                    path: destination.to_path_buf(),
                    source,
                }
            })?;
        }

        temporary
            .as_file()
            .sync_all()
            .map_err(|source| ModrinthError::DownloadWrite {
                path: destination.to_path_buf(),
                source,
            })?;

        let actual_sha512 = hex_digest(hasher.finalize().as_slice());
        let expected_sha512 = file.hashes.sha512.to_ascii_lowercase();
        if actual_sha512 != expected_sha512 {
            return Err(ModrinthError::ChecksumMismatch {
                url: file.url.clone(),
                expected: expected_sha512,
                actual: actual_sha512,
            });
        }

        if file.size != 0 && size != file.size {
            return Err(ModrinthError::SizeMismatch {
                url: file.url.clone(),
                expected: file.size,
                actual: size,
            });
        }

        temporary
            .persist(destination)
            .map_err(|error| ModrinthError::DownloadWrite {
                path: destination.to_path_buf(),
                source: error.error,
            })?;

        Ok(DownloadReport {
            path: destination.to_path_buf(),
            sha512: actual_sha512,
            size,
        })
    }

    fn get_json<T>(&self, path: &str, query: &[(&str, String)]) -> Result<T, ModrinthError>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}{}", self.base_url, path);
        let response =
            self.http
                .get(&url)
                .query(query)
                .send()
                .map_err(|source| ModrinthError::Request {
                    url: url.clone(),
                    source,
                })?;

        let status = response.status();
        let body = response.text().map_err(|source| ModrinthError::Request {
            url: url.clone(),
            source,
        })?;

        if !status.is_success() {
            return Err(ModrinthError::HttpStatus {
                url,
                status: status.as_u16(),
                body,
            });
        }

        serde_json::from_str(&body).map_err(|source| ModrinthError::Decode { url, source })
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use crate::types::Hashes;

    #[test]
    fn rejects_empty_user_agent() {
        assert!(matches!(
            ModrinthClient::new(""),
            Err(ModrinthError::EmptyUserAgent)
        ));
    }

    #[test]
    fn rejects_invalid_user_agent_header() {
        assert!(matches!(
            ModrinthClient::new("bad\nagent"),
            Err(ModrinthError::InvalidUserAgent(_))
        ));
    }

    fn serve_once(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        format!("http://{address}/pack.zip")
    }

    fn sha512_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha512::new();
        hasher.update(bytes);
        hex_digest(hasher.finalize().as_slice())
    }

    fn version_file(url: String, sha512: String, size: u64) -> VersionFile {
        VersionFile {
            filename: "pack.zip".into(),
            hashes: Hashes { sha512, sha1: None },
            url,
            primary: true,
            size,
            file_type: None,
        }
    }

    #[test]
    fn downloads_and_verifies() {
        let body = b"PK fake zip bytes".to_vec();
        let url = serve_once(body.clone());
        let file = version_file(url, sha512_hex(&body), body.len() as u64);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("pack.zip");

        let report = ModrinthClient::new("dpp-test")
            .unwrap()
            .download_file(&file, &destination)
            .unwrap();

        assert_eq!(fs::read(&destination).unwrap(), body);
        assert_eq!(report.size, body.len() as u64);
        assert_eq!(report.sha512, file.hashes.sha512);
    }

    #[test]
    fn rejects_wrong_checksum_without_writing_destination() {
        let body = b"PK fake zip bytes".to_vec();
        let url = serve_once(body.clone());
        let file = version_file(url, "00".into(), body.len() as u64);
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("pack.zip");

        let result = ModrinthClient::new("dpp-test")
            .unwrap()
            .download_file(&file, &destination);

        assert!(matches!(
            result,
            Err(ModrinthError::ChecksumMismatch { .. })
        ));
        assert!(!destination.exists());
    }
}
