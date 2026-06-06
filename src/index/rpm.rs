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


fn repoquery_all(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("dnf")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn update_index(
    index: &mut SonameIndex,
    repo_name: &str,
    config: &crate::types::RepoConfig,
) -> SpmResult<()> {
    let cache_dir = crate::config::paths::repos_cache_dir()
        .join("dnf")
        .join(repo_name);

    if !cache_dir.exists() {
        return Err(SpmError::other(format!(
            "Dnf repo cache '{}' not found. Run spm update first.",
            repo_name
        )));
    }

    let priority = config.effective_priority();

    let provides_text = match repoquery_all(&["repoquery", "--quiet", "--all", "--provides"]) {
        Some(t) => t,
        None => return Ok(()),
    };
    let requires_text = match repoquery_all(&["repoquery", "--quiet", "--all", "--requires"]) {
        Some(t) => t,
        None => return Ok(()),
    };

    // ── Parse all capabilities (provides) ──────────────────────────────
    // capability -> set of packages that provide it
    let mut cap_providers: HashMap<String, Vec<String>> = HashMap::new();
    for line in provides_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (pkg, val) = match line.split_once(" : ") {
            Some((p, v)) => (p.trim(), v.trim()),
            None => continue,
        };
        if let Some(cap) = clean_cap(val) {
            cap_providers
                .entry(cap)
                .or_default()
                .push(pkg.to_string());
        }
    }

    // ── Parse all requires ────────────────────────────────────────────
    // package -> set of capabilities it needs
    let mut pkg_requires: HashMap<String, Vec<String>> = HashMap::new();
    for line in requires_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (pkg, val) = match line.split_once(" : ") {
            Some((p, v)) => (p.trim(), v.trim()),
            None => continue,
        };
        if let Some(cap) = clean_cap(val) {
            pkg_requires
                .entry(pkg.to_string())
                .or_default()
                .push(cap);
        }
    }

    // ── Get version info ──────────────────────────────────────────────
    let version_text = match repoquery_all(&[
        "repoquery",
        "--quiet",
        "--all",
        "--qf",
        "%{NAME}||%{VERSION}||%{RELEASE}",
    ]) {
        Some(t) => t,
        None => return Ok(()),
    };

    let mut pkg_version: HashMap<String, String> = HashMap::new();
    for line in version_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, "||").collect();
        if parts.len() < 2 {
            continue;
        }
        let ver = if parts.len() > 2 {
            format!("{}-{}", parts[1], parts[2])
        } else {
            parts[1].to_string()
        };
        pkg_version.insert(parts[0].to_string(), ver);
    }

    // ── Insert capability entries ─────────────────────────────────────
    // Every capability (SONAME, file path, virtual) gets an entry so the
    // solver can resolve it.
    for (cap, providers) in &cap_providers {
        for pkg_name in providers {
            let version = pkg_version
                .get(pkg_name.as_str())
                .cloned()
                .unwrap_or_default();

            // For SONAME capabilities, store the SONAME-level requires
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
                    source: RepoSource::Dnf,
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
    // Every real package gets an entry with its own requires so the
    // solver can resolve it as a dependency target.
    for (pkg_name, version) in &pkg_version {
        let requires = pkg_requires
            .remove(pkg_name.as_str())
            .unwrap_or_default();

        index.insert_provider(
            pkg_name,
            SonameProvider {
                source: RepoSource::Dnf,
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
