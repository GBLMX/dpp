use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::de::DeserializeOwned;

use crate::types::{ProjectVersion, SearchResponse};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
