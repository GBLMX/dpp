use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub total_hits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub all_project_types: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    #[serde(default)]
    pub icon_url: Option<String>,
}

impl SearchHit {
    /// Modrinth's primary `project_type` is often `mod` even for
    /// datapack-capable projects; use the loader/version data for the real decision.
    pub fn is_datapack_capable(&self) -> bool {
        self.all_project_types.iter().any(|kind| kind == "datapack")
            || self.categories.iter().any(|kind| kind == "datapack")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProjectVersion {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub date_published: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub files: Vec<VersionFile>,
}

impl ProjectVersion {
    pub fn is_datapack(&self) -> bool {
        self.loaders.iter().any(|loader| loader == "datapack")
    }

    pub fn supports_game_version(&self, version: &str) -> bool {
        self.game_versions
            .iter()
            .any(|candidate| candidate == version)
    }

    /// Select a `.zip` datapack artifact, preferring the primary file.
    ///
    /// `.jar` files are modloader artifacts and must not be installed by dpp.
    pub fn datapack_file(&self) -> Option<&VersionFile> {
        if !self.is_datapack() {
            return None;
        }

        self.files
            .iter()
            .filter(|file| is_datapack_zip(&file.filename))
            .find(|file| file.primary)
            .or_else(|| {
                self.files
                    .iter()
                    .find(|file| is_datapack_zip(&file.filename))
            })
    }
}

fn is_datapack_zip(filename: &str) -> bool {
    filename.to_ascii_lowercase().ends_with(".zip")
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionFile {
    pub filename: String,
    pub hashes: Hashes,
    pub url: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub file_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hashes {
    pub sha512: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub dependency_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH_JSON: &str = r#"{
        "hits": [{
            "project_id": "ZVzW5oNS",
            "slug": "incendium",
            "title": "Incendium",
            "description": "A nether expansion",
            "project_type": "mod",
            "all_project_types": ["datapack", "mod"],
            "categories": ["datapack", "worldgen"],
            "versions": ["26.3"],
            "downloads": 12345,
            "follows": 678,
            "icon_url": "https://example.com/icon.png"
        }],
        "offset": 0,
        "limit": 1,
        "total_hits": 1
    }"#;

    const VERSIONS_JSON: &str = r#"[{
        "id": "datapack-version",
        "project_id": "ZVzW5oNS",
        "name": "v5.5.4+26.3 ~ Datapack",
        "version_number": "5.5.4+26.3",
        "dependencies": [{
            "version_id": null,
            "project_id": "dep-id",
            "file_name": null,
            "dependency_type": "required"
        }],
        "game_versions": ["26.3"],
        "loaders": ["datapack"],
        "version_type": "release",
        "status": "listed",
        "date_published": "2026-09-15T00:00:00Z",
        "downloads": 42,
        "files": [{
            "filename": "Incendium.zip",
            "hashes": {"sha512": "abc", "sha1": "def"},
            "url": "https://cdn.modrinth.com/Incendium.zip",
            "primary": true,
            "size": 1234,
            "file_type": null
        }]
    }, {
        "id": "mod-version",
        "project_id": "ZVzW5oNS",
        "name": "v5.5.4+26.3 ~ Mod",
        "version_number": "5.5.4+26.3",
        "dependencies": [],
        "game_versions": ["26.3"],
        "loaders": ["fabric", "neoforge"],
        "version_type": "release",
        "files": [{
            "filename": "Incendium.jar",
            "hashes": {"sha512": "ghi"},
            "url": "https://cdn.modrinth.com/Incendium.jar",
            "primary": true,
            "size": 999
        }]
    }]"#;

    #[test]
    fn parses_search_response() {
        let response: SearchResponse = serde_json::from_str(SEARCH_JSON).unwrap();
        assert_eq!(response.total_hits, 1);
        let hit = &response.hits[0];
        assert_eq!(hit.slug, "incendium");
        assert_eq!(hit.project_type, "mod");
        assert!(hit.is_datapack_capable());
    }

    #[test]
    fn selects_datapack_zip_not_mod_jar() {
        let versions: Vec<ProjectVersion> = serde_json::from_str(VERSIONS_JSON).unwrap();
        let datapack = versions
            .iter()
            .find(|version| version.is_datapack())
            .unwrap();

        assert_eq!(datapack.id, "datapack-version");
        assert!(datapack.supports_game_version("26.3"));
        assert_eq!(datapack.datapack_file().unwrap().filename, "Incendium.zip");
        assert_eq!(datapack.datapack_file().unwrap().hashes.sha512, "abc");
        assert!(versions[1].datapack_file().is_none());
    }

    #[test]
    fn missing_optional_fields_are_tolerated() {
        let hit: SearchHit =
            serde_json::from_str(r#"{"project_id":"x","slug":"x","title":"X"}"#).unwrap();
        assert_eq!(hit.downloads, 0);
        assert!(hit.all_project_types.is_empty());
    }
}
