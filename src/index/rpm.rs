use std::collections::HashMap;

use crate::error::{SpmError, SpmResult};
use crate::types::RepoSource;

use super::{SonameIndex, SonameProvider};

/// Clean a capability name from RPM metadata:
/// - Strip rpm internal tokens (`rpmlib(`, `rtld(GNU_HASH)`)
/// - Strip architecture/version parentheses: `libfoo.so.3()(64bit)` → `libfoo.so.3`
/// - Keep everything else: SONAMEs, file paths (`/usr/bin/perl`), package names
fn clean_cap(val: &str) -> Option<String> {
    let val = val.trim();
    if val.is_empty()
        || val.starts_with("rpmlib(")
        || val == "rtld(GNU_HASH)"
    {
        return None;
    }
    let cleaned = val.split('(').next().unwrap_or(val).trim().to_string();
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

/// Check whether a capability name looks like a SONAME
fn is_soname(val: &str) -> bool {
    val.contains(".so.") || (val.starts_with("lib") && val.contains(".so"))
}


pub fn update_index(
    index: &mut SonameIndex,
    repo_name: &str,
    config: &crate::types::RepoConfig,
) -> SpmResult<()> {
    let cache_dir = crate::config::paths::repos_cache_dir()
        .join("rpm")
        .join(repo_name);

    if !cache_dir.exists() {
        return Err(SpmError::other(format!(
            "Rpm repo cache '{}' not found. Run spm update first.",
            repo_name
        )));
    }

    let priority = config.effective_priority();

    // Read cached repo-index.json for the dnf repo
    let index_path = cache_dir.join("repo-index.json");
    if !index_path.exists() {
        return Err(SpmError::other(format!(
            "No repo-index.json in dnf cache for '{}'. Run spm update first.",
            repo_name
        )));
    }
    let content = std::fs::read_to_string(&index_path)?;
    let repo_index: crate::types::RepoIndex = serde_json::from_str(&content)?;

    // Build capability provides from the repo index
    let mut cap_providers: HashMap<String, Vec<String>> = HashMap::new();
    let mut pkg_requires: HashMap<String, Vec<String>> = HashMap::new();
    let mut pkg_version: HashMap<String, String> = HashMap::new();

    for record in &repo_index.packages {
        pkg_version.insert(record.name.clone(), record.version.clone());

        let clean_deps: Vec<String> = record.dependencies.iter()
            .filter_map(|d| clean_cap(d))
            .collect();
        if !clean_deps.is_empty() {
            pkg_requires.insert(record.name.clone(), clean_deps);
        }

        // Register SONAME provides
        for soname in &record.provides_soname {
            cap_providers
                .entry(soname.clone())
                .or_default()
                .push(record.name.clone());
        }
    }

    // ── Insert capability entries ─────────────────────────────────────
    for (cap, providers) in &cap_providers {
        for pkg_name in providers {
            let version = pkg_version
                .get(pkg_name.as_str())
                .cloned()
                .unwrap_or_default();

            let soname_requires: Vec<String> = if is_soname(cap) {
                pkg_requires
                    .get(pkg_name.as_str())
                    .map(|r| {
                        r.iter()
                            .filter(|d| is_soname(d))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            index.insert_provider(
                cap,
                SonameProvider {
                    source: RepoSource::Rpm,
                    repo: repo_name.to_string(),
                    pkg: pkg_name.clone(),
                    version,
                    priority,
                },
            );
            if !soname_requires.is_empty() && index.get_requires(cap).is_none_or(|r| r.is_empty()) {
                index.set_requires(cap, soname_requires);
            }
        }
    }

    // ── Insert package-name entries ───────────────────────────────────
    for (pkg_name, version) in &pkg_version {
        let requires = pkg_requires
            .remove(pkg_name.as_str())
            .unwrap_or_default();

        index.insert_provider(
            pkg_name,
            SonameProvider {
                source: RepoSource::Rpm,
                repo: repo_name.to_string(),
                pkg: pkg_name.clone(),
                version: version.clone(),
                priority,
            },
        );
        if !requires.is_empty() {
            index.set_requires(pkg_name, requires);
        }
    }

    Ok(())
}
