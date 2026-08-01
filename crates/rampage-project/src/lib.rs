//! Read-only universal project discovery.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTwinV1 {
    pub schema: String,
    pub root: PathBuf,
    pub name: String,
    pub fingerprint: String,
    pub languages: BTreeMap<String, u64>,
    pub toolchains: BTreeSet<String>,
    pub manifests: Vec<PathBuf>,
    pub file_count: u64,
    pub total_bytes: u64,
    pub discovered_at: DateTime<Utc>,
    pub read_only_discovery: bool,
}

pub fn discover_project(path: impl AsRef<Path>) -> anyhow::Result<ProjectTwinV1> {
    let requested = fs::canonicalize(path.as_ref())
        .with_context(|| format!("project path does not exist: {}", path.as_ref().display()))?;
    anyhow::ensure!(requested.is_dir(), "project path must be a directory");
    let root = find_repository_root(&requested).unwrap_or(requested);
    let mut languages = BTreeMap::new();
    let mut toolchains = BTreeSet::new();
    let mut manifests = Vec::new();
    let mut file_count = 0_u64;
    let mut total_bytes = 0_u64;
    let mut fingerprint = Sha256::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(include_entry)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
        let metadata = entry.metadata()?;
        file_count += 1;
        total_bytes = total_bytes.saturating_add(metadata.len());
        fingerprint.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        fingerprint.update(metadata.len().to_be_bytes());
        if let Some(language) = language_for(entry.path()) {
            *languages.entry(language.into()).or_default() += 1;
        }
        if let Some(toolchain) = toolchain_for(relative) {
            toolchains.insert(toolchain.into());
            manifests.push(relative.to_path_buf());
            if metadata.len() <= 1024 * 1024 {
                fingerprint.update(fs::read(entry.path())?);
            }
        }
    }
    manifests.sort();
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string();
    Ok(ProjectTwinV1 {
        schema: "rampage.project-twin.v1".into(),
        root,
        name,
        fingerprint: format!("sha256:{}", hex::encode(fingerprint.finalize())),
        languages,
        toolchains,
        manifests,
        file_count,
        total_bytes,
        discovered_at: Utc::now(),
        read_only_discovery: true,
    })
}

fn find_repository_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        // A drive-root `.git` would turn an unrelated folder into a whole-volume scan.
        .find(|candidate| candidate.parent().is_some() && candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn include_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    !matches!(
        entry.file_name().to_str(),
        Some(".git" | "node_modules" | "target" | ".venv" | "dist" | "build" | ".rampage")
    )
}

fn language_for(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" => Some("javascript"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "go" => Some("go"),
        "cpp" | "cc" | "cxx" | "h" | "hpp" => Some("cpp"),
        "cs" => Some("csharp"),
        _ => None,
    }
}

fn toolchain_for(path: &Path) -> Option<&'static str> {
    match path.file_name()?.to_str()? {
        "Cargo.toml" => Some("cargo"),
        "pyproject.toml" | "uv.lock" => Some("python-uv"),
        "package.json" | "pnpm-lock.yaml" => Some("node-pnpm"),
        "go.mod" => Some("go"),
        "Package.swift" => Some("swiftpm"),
        "build.gradle" | "build.gradle.kts" => Some("gradle"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_stable_read_only_twin_and_skips_build_outputs() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        fs::create_dir(temp.path().join("target")).unwrap();
        fs::write(temp.path().join("target/junk.rs"), "ignored").unwrap();
        let first = discover_project(temp.path()).unwrap();
        let second = discover_project(temp.path()).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.languages.get("rust"), Some(&1));
        assert!(first.toolchains.contains("cargo"));
        assert!(first.read_only_discovery);
    }
}
