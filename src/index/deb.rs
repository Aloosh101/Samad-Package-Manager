use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};

use crate::error::{SpmError, SpmResult};
use crate::types::RepoSource;

use super::{SonameIndex, SonameProvider};

struct DebPkgInfo {
    version: String,
    deps: Vec<String>,
}

pub fn update_index(
    index: &mut SonameIndex,
    repo_name: &str,
    config: &crate::types::RepoConfig,
) -> SpmResult<()> {
    let cache_dir = crate::config::paths::repos_cache_dir()
        .join("apt")
        .join(repo_name);

    if !cache_dir.exists() {
        return Err(SpmError::other(format!(
            "Apt repo cache '{}' not found. Run spm update first.",
            repo_name
        )));
    }

    let pkg_map = parse_packages(&cache_dir)?;
    if pkg_map.is_empty() {
        return Ok(());
    }

    let contents_path = cache_dir.join("Contents-amd64.gz");
    if !contents_path.exists() {
        tracing::debug!(
            "No Contents-amd64.gz for repo '{}', skipping SONAME index",
            repo_name
        );
        return Ok(());
    }

    let priority = config.effective_priority();
    let file = fs::File::open(&contents_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let reader = BufReader::new(decoder);

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (path_part, pkg_name) = match trimmed.split_once(char::is_whitespace) {
            Some((p, n)) => (p.trim(), n.trim()),
            None => continue,
        };

        let soname = match path_part.rsplit('/').next() {
            Some(f) => f,
            None => continue,
        };

        if !soname.contains(".so") {
            continue;
        }

        if let Some(info) = pkg_map.get(pkg_name) {
            index.insert_provider(
                soname,
                SonameProvider {
                    source: RepoSource::Apt,
                    repo: repo_name.to_string(),
                    pkg: pkg_name.to_string(),
                    version: info.version.clone(),
                    priority,
                },
            );
            if index.get_requires(soname).is_none_or(|r| r.is_empty()) {
                index.set_requires(soname, info.deps.clone());
            }
        }
    }

    // Register package-name entries so the solver can resolve package names
    for (pkg_name, info) in &pkg_map {
        index.insert_provider(
            pkg_name,
            SonameProvider {
                source: RepoSource::Apt,
                repo: repo_name.to_string(),
                pkg: pkg_name.clone(),
                version: info.version.clone(),
                priority,
            },
        );
        if index.get_requires(pkg_name).is_none_or(|r| r.is_empty()) {
            index.set_requires(pkg_name, info.deps.clone());
        }
    }

    Ok(())
}

fn process_dep(dep: &str, deps: &mut Vec<String>) {
    let dep = dep.trim();
    // Handle OR alternatives: take the first that isn't empty
    let chosen = if dep.contains('|') {
        dep.split('|')
            .map(|a| a.trim())
            .find(|a| !a.is_empty())
            .unwrap_or("")
    } else {
        dep
    };
    if chosen.is_empty() {
        return;
    }
    let (name, constraint) = crate::backend::parse_dep_entry(chosen);
    if !name.is_empty() {
        if constraint.is_empty() {
            deps.push(name);
        } else {
            deps.push(format!("{name} ({constraint})"));
        }
    }
}

fn parse_packages(cache_dir: &std::path::Path) -> SpmResult<HashMap<String, DebPkgInfo>> {
    let mut pkg_map = HashMap::new();

    let entries = match fs::read_dir(cache_dir) {
        Ok(e) => e,
        Err(_) => return Ok(pkg_map),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if !fname.starts_with("Packages-") {
            continue;
        }

        let content = fs::read_to_string(&path)?;

        let mut current_pkg: Option<String> = None;
        let mut current_ver: Option<String> = None;
        let mut current_deps: Vec<String> = Vec::new();

        for line in content.lines() {
            if line.is_empty() {
                if let (Some(pkg), Some(ver)) = (&current_pkg, &current_ver) {
                    pkg_map.entry(pkg.clone()).or_insert(DebPkgInfo {
                        version: ver.clone(),
                        deps: current_deps.clone(),
                    });
                }
                current_pkg = None;
                current_ver = None;
                current_deps.clear();
                continue;
            }

            if let Some(val) = line.strip_prefix("Package: ") {
                current_pkg = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("Version: ") {
                current_ver = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("Depends: ") {
                for dep in val.split(',') {
                    process_dep(dep, &mut current_deps);
                }
            } else if let Some(val) = line.strip_prefix("Pre-Depends: ") {
                for dep in val.split(',') {
                    process_dep(dep, &mut current_deps);
                }
            }
        }

        if let (Some(pkg), Some(ver)) = (&current_pkg, &current_ver) {
            pkg_map.entry(pkg.clone()).or_insert(DebPkgInfo {
                version: ver.clone(),
                deps: current_deps.clone(),
            });
        }
    }

    Ok(pkg_map)
}
