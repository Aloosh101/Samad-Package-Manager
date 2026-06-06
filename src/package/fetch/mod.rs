pub(crate) mod download;
pub(crate) mod gs;
pub(crate) mod scan;

use std::collections::HashMap;
use chrono::Utc;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::paths;
use crate::error::{SpmError, SpmResult};
use crate::types::*;
use crate::verify::{verify_manifest_signature, SignatureStatus};

pub(crate) struct PackageConflict {
    pub(crate) owner: String,
}

pub(crate) struct FetchedPackage {
    pub(crate) pkg: InstalledPackage,
    pub(crate) extracted_dir: String,
    pub(crate) files: Vec<FileRecord>,
    pub(crate) manifest: Manifest,
    pub(crate) conflicting_packages: Vec<String>,
    pub(crate) scripts: super::scripts::Scripts,
    #[allow(dead_code)]
    pub(crate) _tmp_dir: Option<tempfile::TempDir>,
}

pub(crate) fn fetch_and_extract(name: &str, repo_name: &str, repo_config: &RepoConfig, replace: bool, conn: &rusqlite::Connection) -> SpmResult<FetchedPackage> {
    let tmp_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let tmp_dir = tmp_dir_obj.path().to_string_lossy().to_string();

    let result = match repo_config.source {
        RepoSource::Apt => fetch_apt_to_temp(name, repo_name, repo_config, &tmp_dir),
        RepoSource::Dnf => fetch_dnf_to_temp(name, repo_name, repo_config, &tmp_dir),
        RepoSource::Native => fetch_native_to_temp(name, repo_name, repo_config, &tmp_dir),
    };

    let mut fetched = match result {
        Ok(f) => f,
        Err(primary_err) => {
            tracing::debug!("Primary fetch failed for '{}': {primary_err}. Trying cross-format fallback...", name);
            match fetch_cross_format_fallback(name, &tmp_dir) {
                Ok(f) => f,
                Err(_) => return Err(primary_err),
            }
        }
    };

    // Preserve SAM v2 fields from the original manifest (if available from SAM archive)
    let original_manifest = std::mem::take(&mut fetched.manifest);
    let mut manifest = build_manifest(&fetched.pkg, Some(&fetched.extracted_dir))?;
    if !original_manifest.systemd_units.is_empty()
        || !original_manifest.sysusers.is_empty()
        || !original_manifest.tmpfiles.is_empty()
        || !original_manifest.triggers.is_empty()
        || !original_manifest.obsoletes.is_empty()
        || !original_manifest.conffiles.is_empty()
        || original_manifest.format_version > 1
        || original_manifest.source.is_some()
    {
        // Carry over SAM v2 fields from the original archive manifest
        manifest.systemd_units = original_manifest.systemd_units;
        manifest.sysusers = original_manifest.sysusers;
        manifest.tmpfiles = original_manifest.tmpfiles;
        manifest.triggers = original_manifest.triggers;
        manifest.obsoletes = original_manifest.obsoletes;
        manifest.conffiles = original_manifest.conffiles;
        manifest.format_version = original_manifest.format_version.max(manifest.format_version);
        manifest.source = original_manifest.source.or(manifest.source);
        manifest.signature = original_manifest.signature.or(manifest.signature);
    }
    let (files, conflicts) = scan::scan_extracted_files(conn, name, &fetched.extracted_dir, &fetched.pkg, &tmp_dir)?;
    fetched.files = files;
    fetched.conflicting_packages = collect_conflicting_packages(&conflicts, replace, name);

    let sam_path = paths::sam_archive(name);
    let sam_result = super::sam::create_sam(
        &manifest, &fetched.extracted_dir, None, &sam_path.to_string_lossy(),
    );
    if let Err(e) = sam_result {
        tracing::warn!("Failed to create .sam for {}: {}. Installing without .sam cache.", name, e);
    }
    fetched.manifest = manifest;
    fetched._tmp_dir = Some(tmp_dir_obj);

    fetched.pkg.format = PackageFormat::Sam;

    Ok(fetched)
}

pub(crate) fn fetch_cross_format_fallback(name: &str, tmp_dir: &str) -> SpmResult<FetchedPackage> {
    let apt_output = Command::new(crate::util::backend::resolve("apt-get"))
        .args(["download", name, "--quiet"])
        .current_dir(tmp_dir)
        .output();

    if let Ok(output) = apt_output {
        if output.status.success() {
            if let Ok(entries) = fs::read_dir(tmp_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("deb") {
                        let extract_dir = format!("{}/root", tmp_dir);
                        fs::create_dir_all(&extract_dir)?;
                        super::deb::extract_deb(&p.to_string_lossy(), &extract_dir)?;

                        let version = match super::deb::parse_deb_control(&p.to_string_lossy()) {
                            Ok(pkg) => pkg.version,
                            Err(_) => "unknown".into(),
                        };

                        let manifest = Manifest {
                            name: name.to_string(),
                            version: version.clone(),
                            format_version: 1,
                            source: Some(PackageSource {
                                original_format: "deb".to_string(),
                                original_package: p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                                repo: "cross-format-fallback".to_string(),
                                hash_original: String::new(),
                            }),
                            ..Default::default()
                        };

                        let pkg = InstalledPackage {
                            name: name.to_string(),
                            version,
                            format: PackageFormat::Sam,
                            install_type: InstallType::Native,
                            manifest: Some(serde_json::to_string(&manifest).unwrap_or_default()),
                            install_date: Utc::now().to_rfc3339(),
                            source_repo: Some("cross-format (deb)".to_string()),
                            store_hash: None,
                            origin: InstallOrigin::Spm,
                        };

                        return Ok(FetchedPackage {
                            extracted_dir: extract_dir,
                            files: Vec::new(),
                            manifest,
                            pkg,
                            conflicting_packages: Vec::new(),
                            scripts: super::scripts::Scripts::default(),
                            _tmp_dir: None,
                        });
                    }
                }
            }
        }
    }

    let dnf_output = Command::new(crate::util::backend::resolve("dnf"))
        .args(["download", "--destdir", tmp_dir, name])
        .stderr(Stdio::null())
        .output();

    if let Ok(output) = dnf_output {
        if output.status.success() {
            if let Ok(entries) = fs::read_dir(tmp_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some("rpm") {
                            let extract_dir = format!("{}/root", tmp_dir);
                            fs::create_dir_all(&extract_dir)?;
                            let extract_output = Command::new(crate::util::backend::resolve("rpm2cpio"))
                                .arg(&p)
                                .stdout(Stdio::piped())
                                .stderr(Stdio::piped())
                                .output()
                                .map_err(|e| SpmError::command_failed(format!("rpm2cpio failed: {e}")))?;
                            if !extract_output.status.success() {
                                return Err(SpmError::command_failed("rpm2cpio extraction failed"));
                            }
                            let mut cpio = Command::new(crate::util::backend::resolve("cpio"))
                                .args(["-idmv", "-D", &extract_dir])
                                .stdin(Stdio::piped())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn()
                                .map_err(|e| SpmError::command_failed(format!("Failed to spawn cpio: {e}")))?;
                            if let Some(mut stdin) = cpio.stdin.take() {
                                stdin.write_all(&extract_output.stdout)?;
                            }
                            let cpio_status = cpio.wait_with_output()
                                .map_err(|e| SpmError::command_failed(format!("cpio failed: {e}")))?;
                            if !cpio_status.status.success() {
                                return Err(SpmError::command_failed("cpio extraction failed"));
                            }

                        let version = match super::rpm::parse_rpm_header(&p.to_string_lossy()) {
                            Ok(pkg) => pkg.version,
                            Err(_) => "unknown".into(),
                        };

                        let manifest = Manifest {
                            name: name.to_string(),
                            version: version.clone(),
                            format_version: 1,
                            source: Some(PackageSource {
                                original_format: "rpm".to_string(),
                                original_package: p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                                repo: "cross-format-fallback".to_string(),
                                hash_original: String::new(),
                            }),
                            ..Default::default()
                        };

                        let pkg = InstalledPackage {
                            name: name.to_string(),
                            version,
                            format: PackageFormat::Sam,
                            install_type: InstallType::Native,
                            manifest: Some(serde_json::to_string(&manifest).unwrap_or_default()),
                            install_date: Utc::now().to_rfc3339(),
                            source_repo: Some("cross-format (rpm)".to_string()),
                            store_hash: None,
                            origin: InstallOrigin::Spm,
                        };

                        return Ok(FetchedPackage {
                            extracted_dir: extract_dir,
                            files: Vec::new(),
                            manifest,
                            pkg,
                            conflicting_packages: Vec::new(),
                            scripts: super::scripts::Scripts::default(),
                            _tmp_dir: None,
                        });
                    }
                }
            }
        }
    }

    Err(SpmError::package_not_found(format!(
        "Cross-format fallback: '{name}' not found in any format"
    )))
}

pub(crate) fn collect_conflicting_packages(conflicts: &[PackageConflict], replace: bool, name: &str) -> Vec<String> {
    let mut packages: Vec<String> = Vec::new();
    for c in conflicts {
        if c.owner != name && !packages.contains(&c.owner) {
            packages.push(c.owner.clone());
        }
        if c.owner == name && !replace && !packages.contains(&c.owner) {
            packages.push(c.owner.clone());
        }
    }
    packages
}

pub(crate) fn build_manifest(pkg: &InstalledPackage, extracted_dir: Option<&str>) -> SpmResult<Manifest> {
    let total_size = extracted_dir
        .map(|d| scan::dir_size(Path::new(d)))
        .unwrap_or(0);

    let conffiles = extracted_dir
        .map(scan::detect_conffiles)
        .unwrap_or_default();

    Ok(Manifest {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        architecture: String::new(),
        maintainer: String::new(),
        description: String::new(),
        dependencies: Vec::new(),
        conflicts: Vec::new(),
        provides: Vec::new(),
        recommends: Vec::new(),
        install_size: total_size,
        format_version: 1,
        source: None,
        ai_metadata: None,
        signature: None,
        systemd_units: vec![],
        sysusers: vec![],
        tmpfiles: vec![],
        triggers: vec![],
        obsoletes: vec![],
        conffiles,
    })
}

fn host_arch() -> String {
    let arch = std::env::consts::ARCH;
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        other => other,
    }.to_string()
}

pub(crate) fn fetch_apt_to_temp(
    name: &str,
    repo_name: &str,
    config: &RepoConfig,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    if config.mirrors.is_some() {
        let http_result = fetch_apt_http_to_temp(name, repo_name, config, tmp_dir);
        if http_result.is_ok() {
            return http_result;
        }
    }
    fetch_apt_get_to_temp(name, repo_name, config, tmp_dir)
}

fn fetch_apt_get_to_temp(
    name: &str,
    _repo_name: &str,
    _config: &RepoConfig,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    let cache_dir = paths::archives_dir();
    fs::create_dir_all(&cache_dir)?;

    let output = download::run_with_progress(
        Command::new(crate::util::backend::resolve("apt-cache"))
            .args(["show", name]),
        &format!("🔍 Searching for '{name}' in apt repositories"),
    ).map_err(|e| SpmError::command_failed(format!("Failed to run apt-cache: {e}. Is apt available?")))?;

    if !output.status.success() {
        return Err(SpmError::package_not_found(format!(
            "Package '{name}' not found in apt repositories"
        )));
    }

    let info = String::from_utf8_lossy(&output.stdout);
    let mut version = String::new();
    let mut _arch = String::new();

    for line in info.lines() {
        if let Some(v) = line.strip_prefix("Version: ") {
            version = v.trim().to_string();
        }
        if let Some(a) = line.strip_prefix("Architecture: ") {
            _arch = a.trim().to_string();
        }
    }

    let download_output = Command::new(crate::util::backend::resolve("apt-get"))
        .args(["download", name, "--quiet"])
        .current_dir(&cache_dir)
        .output()
        .map_err(|e| SpmError::command_failed(format!("apt-get download failed: {e}")))?;

    if !download_output.status.success() {
        let stderr = String::from_utf8_lossy(&download_output.stderr);
        return Err(SpmError::other(format!("apt-get could not download '{name}': {stderr}")));
    }

    let deb_path = download::find_deb_in_cache(cache_dir.to_string_lossy().as_ref(), name)
        .map_err(|_| SpmError::other(format!(
            "apt-get downloaded '{name}' but no .deb file found in cache"
        )))?;

    let extracted_dir = format!("{}/root", tmp_dir);
    super::deb::extract_deb(&deb_path, &extracted_dir)?;

    let scripts_dir = format!("{}/scripts", tmp_dir);
    let scripts = super::scripts::extract_deb_scripts(&deb_path, &scripts_dir)?;

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: name.to_string(),
            version: version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: name.to_string(),
            version,
            format: PackageFormat::Deb,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some("apt".to_string()),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

fn fetch_apt_http_to_temp(
    name: &str,
    repo_name: &str,
    config: &RepoConfig,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    let arches = [host_arch()];
    let mut version = String::new();
    let mut filename = String::new();
    let mut sha256 = None;

    let cache_base = crate::config::paths::repos_cache_dir().join("apt").join(repo_name);

    for arch in &arches {
        if let Some(components) = &config.components {
            for component in components {
                let cache_name = format!("Packages-{}-{}-{}",
                    config.codename.as_deref().unwrap_or("unknown"),
                    component,
                    arch,
                );
                let cache_path = cache_base.join(&cache_name);
                if !cache_path.exists() {
                    continue;
                }
                if let Ok(text) = fs::read_to_string(&cache_path) {
                    if let Some(entry) = parse_deb822_entry(&text, name) {
                        if let Some(v) = entry.get("Version") {
                            version = v.clone();
                        }
                        if let Some(f) = entry.get("Filename") {
                            filename = f.clone();
                        }
                        if let Some(sha_field) = entry.get("SHA256") {
                            for line in sha_field.lines() {
                                let parts: Vec<&str> = line.split_whitespace().collect();
                                if parts.len() >= 3 && *parts[2] == *filename {
                                    sha256 = Some(parts[0].to_string());
                                    break;
                                }
                            }
                        }
                        if !version.is_empty() && !filename.is_empty() {
                            break;
                        }
                    }
                }
            }
            if !filename.is_empty() {
                break;
            }
        }
    }

    if version.is_empty() || filename.is_empty() {
        return Err(SpmError::package_not_found(format!(
            "Package '{name}' not found in cached apt repository '{}'. Try 'spm update' first.",
            repo_name,
        )));
    }

    let deb_url = if let Some(mirrors) = &config.mirrors {
        let base = mirrors[0].trim_end_matches('/');
        format!("{base}/{filename}")
    } else {
        return Err(SpmError::config(format!(
            "No mirrors configured for apt repo '{}'", repo_name
        )));
    };

    let cache_dir = paths::archives_dir();
    fs::create_dir_all(&cache_dir)?;

    let deb_filename = Path::new(&filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let deb_path = cache_dir.join(deb_filename);

    crate::output::step_info(format!("Downloading '{name}' from {deb_url}"));
    let body = crate::config::repos::fetch_with_retry(&deb_url, 3)?;

    if let Some(ref expected) = sha256 {
        let actual = crate::util::hash::sha256_hex(&body);

        if actual != *expected {
            return Err(SpmError::other(format!(
                "SHA256 mismatch for '{name}': expected {expected}, got {actual}"
            )));
        }
        tracing::debug!("SHA256 verified for '{}'", name);
    }

    fs::write(&deb_path, &body)?;

    let extracted_dir = format!("{}/root", tmp_dir);
    super::deb::extract_deb(&deb_path.to_string_lossy(), &extracted_dir)?;

    let scripts_dir = format!("{}/scripts", tmp_dir);
    let scripts = super::scripts::extract_deb_scripts(&deb_path.to_string_lossy(), &scripts_dir)?;

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: name.to_string(),
            version: version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: name.to_string(),
            version,
            format: PackageFormat::Deb,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some(format!("apt-http:{}", repo_name)),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

fn parse_deb822_entry(text: &str, name: &str) -> Option<HashMap<String, String>> {
    let mut entry = HashMap::new();
    let mut current_key = String::new();
    let mut current_value = String::new();

    for line in text.lines() {
        if line.is_empty() {
            if entry.get("Package").is_some_and(|s: &String| s.eq_ignore_ascii_case(name)) {
                if !current_key.is_empty() {
                    entry.insert(current_key.clone(), current_value.trim().to_string());
                }
                return Some(entry);
            }
            entry.clear();
            current_key.clear();
            current_value.clear();
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            current_value.push(' ');
            current_value.push_str(line.trim());
        } else if let Some((k, v)) = line.split_once(':') {
            if !current_key.is_empty() {
                entry.insert(current_key.clone(), current_value.trim().to_string());
            }
            current_key = k.trim().to_string();
            current_value = v.trim().to_string();
        }
    }

    if !current_key.is_empty() {
        entry.insert(current_key, current_value.trim().to_string());
    }
    if entry.get("Package").is_some_and(|s| s.eq_ignore_ascii_case(name)) {
        return Some(entry);
    }

    None
}

pub(crate) fn fetch_dnf_to_temp(
    name: &str,
    repo_name: &str,
    config: &RepoConfig,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    // Try HTTP direct download — from configured URL or auto-detect from system DNF repos
    let repo_url = config.url.clone().or_else(|| {
        let cache_dir = paths::repos_cache_dir().join("dnf").join(repo_name);
        detect_dnf_baseurl(&cache_dir)
    });
    if let Some(ref url) = repo_url {
        let result = fetch_dnf_http_to_temp(name, repo_name, url, tmp_dir);
        if result.is_ok() {
            return result;
        }
        tracing::debug!("HTTP direct fetch failed for '{name}' via {url}, falling back to dnf CLI");
    }

    let cache_dir = paths::archives_dir();
    fs::create_dir_all(&cache_dir)?;

    let output = download::run_with_progress(
        Command::new(crate::util::backend::resolve("dnf"))
            .args(["repoquery", "--info", name]),
        &format!("🔍 Searching for '{name}' in dnf repositories"),
    ).map_err(|e| SpmError::command_failed(format!("Failed to run dnf: {e}. Is dnf available?")))?;

    if !output.status.success() {
        return Err(SpmError::package_not_found(format!(
            "Package '{name}' not found in dnf repositories"
        )));
    }

    let info = String::from_utf8_lossy(&output.stdout);
    let mut version = String::new();
    let mut arch = String::new();

    for line in info.lines() {
        if let Some(val) = line.split_once(':').map(|x| x.1) {
            let key = line.split(':').next().unwrap_or("").trim();
            let value = val.trim();
            if key == "Version" {
                version = value.to_string();
            } else if key == "Architecture" {
                arch = value.to_string();
            }
        }
    }

    let download_url = resolve_dnf_download_url(name)?;
    let rpm_filename = format!("{}-{}.{}.rpm", name, version, arch);
    let rpm_path = cache_dir.join(&rpm_filename);
    download::retry_download_resumable(&download_url, &rpm_path, name)?;
    let actual_path = rpm_path.to_string_lossy().to_string();

    let extracted_dir = format!("{}/root", tmp_dir);
    let extract_output = Command::new("rpm2cpio")
        .arg(&actual_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run rpm2cpio: {e}")))?;
    if !extract_output.status.success() {
        let stderr = String::from_utf8_lossy(&extract_output.stderr);
        return Err(SpmError::command_failed(format!(
            "rpm2cpio failed: {stderr}"
        )));
    }
    let cpio = Command::new("cpio")
        .args(["-idmv", "-D", &extracted_dir])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SpmError::command_failed(format!("Failed to spawn cpio: {e}")))?;
    let mut cpio_stdin = cpio.stdin
        .as_ref()
        .ok_or_else(|| SpmError::other("cpio stdin not available".to_string()))?;
    cpio_stdin
        .write_all(&extract_output.stdout)
        .map_err(|e| SpmError::other(format!("Failed to pipe to cpio: {e}")))?;
    let cpio_status = cpio.wait_with_output()
        .map_err(|e| SpmError::command_failed(format!("cpio failed: {e}")))?;
    if !cpio_status.status.success() {
        return Err(SpmError::command_failed("cpio extraction failed".to_string()));
    }

    let scripts = super::scripts::extract_rpm_scripts(&actual_path)?;

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: name.to_string(),
            version: version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: name.to_string(),
            version,
            format: PackageFormat::Rpm,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some("dnf".to_string()),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

/// Scan cached DNF repo config files for a baseurl to use for HTTP direct download
fn detect_dnf_baseurl(cache_dir: &Path) -> Option<String> {
    let dir = cache_dir.to_path_buf();
    if !dir.is_dir() {
        return None;
    }
    for entry in std::fs::read_dir(&dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("repo") {
            continue;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(val) = trimmed.strip_prefix("baseurl=") {
                let url = val.trim().trim_matches('"');
                if url.starts_with("http://") || url.starts_with("https://") {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

fn fetch_dnf_http_to_temp(
    name: &str,
    repo_name: &str,
    repo_url: &str,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    tracing::debug!("Fetching '{name}' via HTTP from RPM-MD repo {repo_url}");

    let rpm_bytes = crate::package::fetch::gs::download_rpm_from_repo(repo_url, name)?;

    let extracted_dir = format!("{}/root", tmp_dir);
    fs::create_dir_all(&extracted_dir)?;

    let cache_dir = paths::archives_dir();
    fs::create_dir_all(&cache_dir)?;
    let rpm_path = cache_dir.join(format!("{name}.rpm"));
    fs::write(&rpm_path, &rpm_bytes)?;

    let extract_output = Command::new("rpm2cpio")
        .arg(&rpm_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run rpm2cpio: {e}")))?;
    if !extract_output.status.success() {
        return Err(SpmError::command_failed("rpm2cpio extraction failed"));
    }
    let mut cpio = Command::new("cpio")
        .args(["-idmv", "-D", &extracted_dir])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SpmError::command_failed(format!("Failed to spawn cpio: {e}")))?;
    if let Some(mut stdin) = cpio.stdin.take() {
        stdin.write_all(&extract_output.stdout)
            .map_err(|e| SpmError::other(format!("Failed to pipe to cpio: {e}")))?;
    }
    let cpio_status = cpio.wait_with_output()
        .map_err(|e| SpmError::command_failed(format!("cpio failed: {e}")))?;
    if !cpio_status.status.success() {
        return Err(SpmError::command_failed("cpio extraction failed"));
    }

    let scripts = super::scripts::extract_rpm_scripts(&rpm_path.to_string_lossy())?;

    let version = match super::rpm::parse_rpm_header(&rpm_path.to_string_lossy()) {
        Ok(pkg) => pkg.version,
        Err(_) => "unknown".into(),
    };

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: name.to_string(),
            version: version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: name.to_string(),
            version,
            format: PackageFormat::Rpm,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some(format!("dnf-http:{}", repo_name)),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

pub(crate) fn fetch_native_to_temp(
    name: &str,
    _repo_name: &str,
    config: &RepoConfig,
    tmp_dir: &str,
) -> SpmResult<FetchedPackage> {
    let url = config.url.as_ref()
        .ok_or_else(|| SpmError::config(format!("Native repository {_repo_name} has no URL configured")))?;

    let download_url = format!("{}/{}.sam", url.trim_end_matches('/'), name);
    let cache_dir = paths::archives_dir();
    fs::create_dir_all(&cache_dir)?;
    let sam_path = cache_dir.join(format!("{name}.sam"));

    tracing::debug!("Downloading SAM from {download_url}");
    download::retry_download_resumable(&download_url, &sam_path, name)?;

    let metadata = fs::metadata(&sam_path)?;
    tracing::debug!("Downloaded {} bytes", metadata.len());

    tracing::debug!("Extracting SAM from {}", sam_path.display());
    let (manifest, _extracted) = super::sam::extract_sam(&sam_path.to_string_lossy(), tmp_dir)?;
    tracing::debug!("Extracted SAM: {} {}", manifest.name, manifest.version);

    match verify_manifest_signature(&manifest)? {
        SignatureStatus::Valid => {
            tracing::debug!("Signature verified for {} (key: {:?})", manifest.name, manifest.signature.as_ref().map(|s| &s.key_id));
        }
        SignatureStatus::NoKey(ref kid) => {
            if std::env::var("SPM_UNSAFE").as_deref() == Ok("1") {
                tracing::warn!("SPM_UNSAFE=1: bypassing signature check for key '{}'", kid);
            } else {
                return Err(SpmError::other(format!(
                    "Package '{}' is signed with key '{}' which is not in trusted keys store. \
                     Set SPM_UNSAFE=1 or import the key to bypass.",
                    manifest.name, kid,
                )));
            }
        }
        SignatureStatus::NotSigned => {
            tracing::debug!("Package '{}' is not signed.", manifest.name);
        }
    }

    let scripts_dir = format!("{}/scripts", tmp_dir);
    let scripts = super::scripts::extract_sam_scripts(&sam_path.to_string_lossy(), &scripts_dir)?;

    Ok(FetchedPackage {
        extracted_dir: tmp_dir.to_string(),
        files: Vec::new(),
        manifest: manifest.clone(),
        pkg: InstalledPackage {
            name: name.to_string(),
            version: manifest.version.clone(),
            format: PackageFormat::Sam,
            install_type: InstallType::Native,
            manifest: Some(serde_json::to_string(&manifest)?),
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some("native".to_string()),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

fn resolve_dnf_download_url(name: &str) -> SpmResult<String> {
    let output = Command::new(crate::util::backend::resolve("dnf"))
        .args(["download", "--urls", name])
        .output()
        .map_err(|e| SpmError::command_failed(format!("dnf download --urls failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpmError::other(format!(
            "dnf could not resolve download URL for '{name}': {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("http://") || line.starts_with("https://") {
            return Ok(line.to_string());
        }
    }
    Err(SpmError::other(format!(
        "Could not parse download URL for '{name}' from dnf output:\n{stdout}"
    )))
}

pub(crate) fn fetch_local_to_temp(path_str: &str, tmp_dir: &str) -> SpmResult<FetchedPackage> {
    let path = Path::new(path_str);
    if !path.exists() {
        return Err(SpmError::invalid_format(format!("File not found: {}", path.display())));
    }

    let extension = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "deb" => fetch_local_deb_to_temp(path, tmp_dir),
        "rpm" => fetch_local_rpm_to_temp(path, tmp_dir),
        "sam" => fetch_local_sam_to_temp(path, tmp_dir),
        _ => Err(SpmError::invalid_format(format!(
            "Unsupported package format: .{}. Supported formats: .deb, .rpm, .sam",
            extension
        ))),
    }
}

fn fetch_local_deb_to_temp(deb_path: &Path, tmp_dir: &str) -> SpmResult<FetchedPackage> {
    let path_str = deb_path.to_string_lossy();
    let pkg_info = super::deb::parse_deb_control(&path_str)?;

    let extracted_dir = format!("{}/root", tmp_dir);
    super::deb::extract_deb(&path_str, &extracted_dir)?;

    let scripts_dir = format!("{}/scripts", tmp_dir);
    let scripts = super::scripts::extract_deb_scripts(&path_str, &scripts_dir)?;

    let filename = deb_path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&path_str);

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: pkg_info.name.clone(),
            version: pkg_info.version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: pkg_info.name,
            version: pkg_info.version,
            format: PackageFormat::Deb,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some(format!("local:{}", filename)),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

fn fetch_local_rpm_to_temp(rpm_path: &Path, tmp_dir: &str) -> SpmResult<FetchedPackage> {
    let path_str = rpm_path.to_string_lossy();
    let pkg_info = super::rpm::parse_rpm_header(&path_str)?;

    let extracted_dir = format!("{}/root", tmp_dir);
    fs::create_dir_all(&extracted_dir)?;

    let extract_output = Command::new(crate::util::backend::resolve("rpm2cpio"))
        .arg(rpm_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| SpmError::command_failed(format!("rpm2cpio failed: {e}")))?;
    if !extract_output.status.success() {
        return Err(SpmError::command_failed("rpm2cpio extraction failed"));
    }

    let mut cpio = Command::new(crate::util::backend::resolve("cpio"))
        .args(["-idmv", "-D", &extracted_dir])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SpmError::command_failed(format!("Failed to spawn cpio: {e}")))?;
    if let Some(mut stdin) = cpio.stdin.take() {
        stdin.write_all(&extract_output.stdout)
            .map_err(|e| SpmError::other(format!("Failed to pipe to cpio: {e}")))?;
    }
    let cpio_status = cpio.wait_with_output()
        .map_err(|e| SpmError::command_failed(format!("cpio failed: {e}")))?;
    if !cpio_status.status.success() {
        return Err(SpmError::command_failed("cpio extraction failed"));
    }

    let scripts = super::scripts::extract_rpm_scripts(&path_str)?;

    let filename = rpm_path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&path_str);

    Ok(FetchedPackage {
        extracted_dir,
        files: Vec::new(),
        manifest: Manifest {
            name: pkg_info.name.clone(),
            version: pkg_info.version.clone(),
            ..Manifest::default()
        },
        pkg: InstalledPackage {
            name: pkg_info.name,
            version: pkg_info.version,
            format: PackageFormat::Rpm,
            install_type: InstallType::Native,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some(format!("local:{}", filename)),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

fn fetch_local_sam_to_temp(sam_path: &Path, tmp_dir: &str) -> SpmResult<FetchedPackage> {
    let path_str = sam_path.to_string_lossy();
    let (manifest, _extracted_files) = super::sam::extract_sam(&path_str, tmp_dir)?;

    let scripts_dir = format!("{}/scripts", tmp_dir);
    let scripts = super::scripts::extract_sam_scripts(&path_str, &scripts_dir)?;

    let filename = sam_path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&path_str);

    Ok(FetchedPackage {
        extracted_dir: tmp_dir.to_string(),
        files: Vec::new(),
        manifest: manifest.clone(),
        pkg: InstalledPackage {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            format: PackageFormat::Sam,
            install_type: InstallType::Native,
            manifest: Some(serde_json::to_string(&manifest)?),
            install_date: Utc::now().to_rfc3339(),
            source_repo: Some(format!("local:{}", filename)),
            store_hash: None,
            origin: InstallOrigin::Spm,
        },
        conflicting_packages: Vec::new(),
        scripts,
        _tmp_dir: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_deb822_entry_simple() {
        let text = "Package: nginx\nVersion: 1.27.0\nArchitecture: amd64\n\n";
        let result = parse_deb822_entry(text, "nginx");
        assert!(result.is_some());
        let map = result.unwrap();
        assert_eq!(map.get("Package").unwrap(), "nginx");
        assert_eq!(map.get("Version").unwrap(), "1.27.0");
        assert_eq!(map.get("Architecture").unwrap(), "amd64");
    }

    #[test]
    fn test_parse_deb822_entry_not_found() {
        let text = "Package: nginx\nVersion: 1.27.0\n\n";
        let result = parse_deb822_entry(text, "apache");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_deb822_entry_multi_entry() {
        let text = "Package: apache\nVersion: 2.0\n\nPackage: nginx\nVersion: 1.27.0\nArchitecture: amd64\n\n";
        let result = parse_deb822_entry(text, "nginx");
        assert!(result.is_some());
        let map = result.unwrap();
        assert_eq!(map.get("Version").unwrap(), "1.27.0");
    }

    #[test]
    fn test_parse_deb822_entry_multiline_value() {
        let text = "Package: nginx\nDescription: A high-performance web\n server and reverse proxy\n\n";
        let result = parse_deb822_entry(text, "nginx");
        assert!(result.is_some());
        let map = result.unwrap();
        let desc = map.get("Description").unwrap();
        assert!(desc.contains("web server"));
    }

    #[test]
    fn test_parse_deb822_entry_case_insensitive() {
        let text = "Package: Nginx\nVersion: 1.0\n\n";
        let result = parse_deb822_entry(text, "nginx");
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_deb822_entry_empty() {
        let result = parse_deb822_entry("", "pkg");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_deb822_entry_no_trailing_newline() {
        let text = "Package: nginx\nVersion: 1.0";
        let result = parse_deb822_entry(text, "nginx");
        assert!(result.is_some());
        assert_eq!(result.unwrap().get("Version").unwrap(), "1.0");
    }

    #[test]
    fn test_collect_conflicting_packages_no_replace() {
        let conflicts = [
            PackageConflict { owner: "old-pkg".into() },
            PackageConflict { owner: "other-pkg".into() },
        ];
        let result = collect_conflicting_packages(&conflicts, false, "new-pkg");
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"old-pkg".to_string()));
        assert!(result.contains(&"other-pkg".to_string()));
    }

    #[test]
    fn test_collect_conflicting_packages_with_replace_skips_self() {
        let conflicts = [
            PackageConflict { owner: "self-pkg".into() },
        ];
        let result = collect_conflicting_packages(&conflicts, true, "self-pkg");
        assert!(result.is_empty(), "replace=true should skip self-conflict");
    }

    #[test]
    fn test_collect_conflicting_packages_with_replace_includes_others() {
        let conflicts = [
            PackageConflict { owner: "other-pkg".into() },
        ];
        let result = collect_conflicting_packages(&conflicts, true, "new-pkg");
        assert_eq!(result.len(), 1, "replace=true should still include other packages");
        assert!(result.contains(&"other-pkg".to_string()));
    }

    #[test]
    fn test_collect_conflicting_packages_self_conflict_without_replace() {
        let conflicts = [
            PackageConflict { owner: "self-pkg".into() },
        ];
        let result = collect_conflicting_packages(&conflicts, false, "self-pkg");
        assert_eq!(result.len(), 1, "should include self when replace=false");
    }

    #[test]
    fn test_collect_conflicting_packages_dedup() {
        let conflicts = [
            PackageConflict { owner: "pkg-a".into() },
            PackageConflict { owner: "pkg-a".into() },
        ];
        let result = collect_conflicting_packages(&conflicts, true, "new-pkg");
        assert_eq!(result.len(), 1, "should deduplicate");
    }
}
