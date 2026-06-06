use std::fs;

use crate::error::{SpmError, SpmResult};
use crate::types::RepoSource;

use super::{SonameIndex, SonameProvider};

pub fn update_index(
    index: &mut SonameIndex,
    repo_name: &str,
    config: &crate::types::RepoConfig,
) -> SpmResult<()> {
    let cache_path = crate::config::paths::repos_cache_dir()
        .join("native")
        .join(repo_name)
        .join("repo-index.json");

    if !cache_path.exists() {
        return Err(SpmError::other(format!(
            "Native repo index '{}' not found. Run spm update first.",
            repo_name
        )));
    }

    let content = fs::read_to_string(&cache_path)?;
    let repo_index: crate::types::RepoIndex = serde_json::from_str(&content)?;

    let priority = config.effective_priority();

    for record in &repo_index.packages {
        let provides_sonames: Vec<String> = if !record.provides_soname.is_empty() {
            record.provides_soname.clone()
        } else {
            // Fallback: extract SONAME-like dependency names
            record
                .dependencies
                .iter()
                .filter(|d| {
                    let d = d.trim();
                    d.contains(".so.") || (d.starts_with("lib") && d.contains(".so"))
                })
                .map(|d| d.trim().to_string())
                .collect()
        };

        if provides_sonames.is_empty() {
            continue;
        }

        for soname in &provides_sonames {
            index.insert_provider(
                soname,
                SonameProvider {
                    source: RepoSource::Native,
                    repo: repo_name.to_string(),
                    pkg: record.name.clone(),
                    version: record.version.clone(),
                    priority,
                },
            );
            let soname_requires: Vec<String> = record
                .dependencies
                .iter()
                .filter(|d| {
                    let d = d.trim();
                    d.contains(".so.") || (d.starts_with("lib") && d.contains(".so"))
                })
                .map(|d| d.trim().to_string())
                .collect();
            if !soname_requires.is_empty() {
                index.set_requires(soname, soname_requires);
            }
        }
    }

    // Register package-name entries so the solver can resolve package names
    for record in &repo_index.packages {
        let deps: Vec<String> = record
            .dependencies
            .iter()
            .map(|d| d.trim().to_string())
            .collect();

        index.insert_provider(
            &record.name,
            SonameProvider {
                source: RepoSource::Native,
                repo: repo_name.to_string(),
                pkg: record.name.clone(),
                version: record.version.clone(),
                priority,
            },
        );
        index.set_requires(&record.name, deps);
    }

    Ok(())
}
