use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};
use crate::types::{RepoConfig, RepoIndex, RepoSource};

const SIGNING_DIR_NAME: &str = "signing-keys";

pub fn load_repos() -> SpmResult<Vec<(String, RepoConfig)>> {
    let repos_dir = paths::repos_config_dir();
    let mut repos = Vec::new();

    tracing::debug!("load_repos: repos_dir={:?} exists={}", repos_dir, repos_dir.exists());

    if !repos_dir.exists() {
        return Ok(repos);
    }

    let read_dir = fs::read_dir(&repos_dir).map_err(|e| {
        SpmError::config(format!("Failed to read repos.d: {e}"))
    })?;
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("list") {
            let content = fs::read_to_string(&path).map_err(|e| {
                SpmError::config(format!("Failed to read {:?}: {e}", path))
            })?;
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
            match toml::from_str::<RepoConfig>(&content) {
                Ok(config) => repos.push((stem.to_string(), config)),
                Err(e) => {
                    tracing::warn!("Failed to parse repo config {:?}: {}", path, e);
                }
            }
        }
    }

    repos.sort_by_key(|(_, rc)| rc.effective_priority());
    Ok(repos)
}

pub fn add_repo(name: &str, source: RepoSource, url: Option<String>, mirrors: Option<Vec<String>>, priority: Option<u32>) -> SpmResult<()> {
    validate_repo_name(name)?;
    let repos_dir = paths::repos_config_dir();
    fs::create_dir_all(&repos_dir)
        .map_err(|e| SpmError::config(format!("Cannot create repos.d: {e}")))?;

    let config = RepoConfig {
        source,
        priority,
        distro: None,
        codename: None,
        components: None,
        mirrors,
        release: None,
        repos: None,
        url,
        signing_key: None,
    };

    let toml_str = toml::to_string(&config)
        .map_err(|e| SpmError::config(format!("Failed to serialize repo config: {e}")))?;

    let path = repos_dir.join(format!("{}.list", name));
    let mut file = fs::File::create(&path)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", path)))?;
    file.write_all(toml_str.as_bytes())
        .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", path)))?;

    Ok(())
}

pub fn create_repo(
    name: &str,
    source: RepoSource,
    path: Option<&str>,
    codename: Option<&str>,
    component: Option<&str>,
    mirror: Option<&str>,
) -> SpmResult<()> {
    validate_repo_name(name)?;

    let base_path = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => paths::db_base().join("repos").join(source.to_string()).join(name),
    };

    match source {
        RepoSource::Native => create_native_repo(name, &base_path),
        RepoSource::Deb => create_deb_repo(name, &base_path, codename, component, mirror),
        RepoSource::Rpm => create_rpm_repo(name, &base_path),
    }
}

fn create_native_repo(name: &str, base_path: &std::path::Path) -> SpmResult<()> {
    fs::create_dir_all(base_path)
        .map_err(|e| SpmError::config(format!("Cannot create repo dir {:?}: {e}", base_path)))?;

    let index = crate::types::RepoIndex {
        repo_name: name.to_string(),
        format_version: 1,
        packages: Vec::new(),
    };
    let index_path = base_path.join("spm-repo.json");
    let json = serde_json::to_string_pretty(&index)
        .map_err(|e| SpmError::config(format!("Failed to serialise repo index: {e}")))?;
    fs::write(&index_path, &json)
        .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", index_path)))?;

    crate::output::step_success(format!("Initialised native repo at {:?}", base_path));

    // Auto-add as a configured repository if not already present
    let repos = load_repos()?;
    if !repos.iter().any(|(n, _)| n == name) {
        let repo_url = format!("file://{}", base_path.to_string_lossy());
        add_repo(name, RepoSource::Native, Some(repo_url.clone()), None, None)?;
        crate::output::step_success(format!("Added '{}' to repository list (url={})", name, repo_url));
    }

    Ok(())
}

fn create_deb_repo(
    name: &str,
    base_path: &std::path::Path,
    codename: Option<&str>,
    component: Option<&str>,
    mirror: Option<&str>,
) -> SpmResult<()> {
    let codename = codename.unwrap_or("stable");
    let component = component.unwrap_or("main");
    let arch = "amd64";

    // pool/ — where .deb files go
    let pool_dir = base_path.join("pool").join(component);
    fs::create_dir_all(&pool_dir)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", pool_dir)))?;

    // dists/<codename>/<component>/binary-<arch>/ — metadata
    let dists_binary = base_path.join("dists").join(codename).join(component).join(format!("binary-{arch}"));
    fs::create_dir_all(&dists_binary)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", dists_binary)))?;

    // Empty Packages file (will be populated by spm repo publish or manual addition)
    let packages_path = dists_binary.join("Packages");
    if !packages_path.exists() {
        fs::write(&packages_path, "")
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", packages_path)))?;
    }

    // Compressed version
    let packages_gz_path = dists_binary.join("Packages.gz");
    if !packages_gz_path.exists() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"").map_err(|e| SpmError::compression(e.to_string()))?;
        let compressed = encoder.finish().map_err(|e| SpmError::compression(e.to_string()))?;
        fs::write(&packages_gz_path, &compressed)
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", packages_gz_path)))?;
    }

    // Release file
    let release_path = base_path.join("dists").join(codename).join("Release");
    if !release_path.exists() {
        use std::io::Write;
        let release_content = format!(
            "Codename: {codename}\nComponents: {component}\nArchitectures: {arch}\n"
        );
        let mut f = fs::File::create(&release_path)
            .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", release_path)))?;
        f.write_all(release_content.as_bytes())
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", release_path)))?;
    }

    crate::output::step_success(format!("Initialised deb repo at {:?}", base_path));
    crate::output::step_info("Structure: pool/ → .deb files, dists/ → metadata");

    // Auto-add as a configured repository
    let repos = load_repos()?;
    if !repos.iter().any(|(n, _)| n == name) {
        let repo_url = format!("file://{}", base_path.to_string_lossy());
        let mirrors = mirror.map(|m| vec![m.to_string()]);
        add_repo(name, RepoSource::Deb, Some(repo_url), mirrors, None)?;
        crate::output::step_success(format!("Added '{}' to repository list", name));
    }

    Ok(())
}

fn create_rpm_repo(name: &str, base_path: &std::path::Path) -> SpmResult<()> {
    // Packages/ — where .rpm files go
    let packages_dir = base_path.join("Packages");
    fs::create_dir_all(&packages_dir)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", packages_dir)))?;

    // repodata/ — metadata
    let repodata_dir = base_path.join("repodata");
    fs::create_dir_all(&repodata_dir)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", repodata_dir)))?;

    // Create a minimal repomd.xml
    let repomd_path = repodata_dir.join("repomd.xml");
    if !repomd_path.exists() {
        let repomd_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd>
  <data type="primary">
    <location href="repodata/primary.xml.gz"/>
    <checksum type="sha256">e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855</checksum>
    <open-checksum type="sha256">e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855</open-checksum>
  </data>
</repomd>
"#;
        fs::write(&repomd_path, repomd_content)
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", repomd_path)))?;
    }

    // Empty primary.xml.gz
    let primary_path = repodata_dir.join("primary.xml.gz");
    if !primary_path.exists() {
        let empty_primary = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata xmlns="http://linux.duke.edu/metadata/common" packages="0"/>
"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(empty_primary.as_bytes()).map_err(|e| SpmError::compression(e.to_string()))?;
        let compressed = encoder.finish().map_err(|e| SpmError::compression(e.to_string()))?;
        fs::write(&primary_path, &compressed)
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", primary_path)))?;
    }

    // Empty filelists.xml.gz
    let filelists_path = repodata_dir.join("filelists.xml.gz");
    if !filelists_path.exists() {
        let empty = r#"<?xml version="1.0" encoding="UTF-8"?>
<filelists xmlns="http://linux.duke.edu/metadata/filelists" packages="0"/>
"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(empty.as_bytes()).map_err(|e| SpmError::compression(e.to_string()))?;
        let compressed = encoder.finish().map_err(|e| SpmError::compression(e.to_string()))?;
        fs::write(&filelists_path, &compressed)
            .map_err(|e| SpmError::config(format!("Cannot write {:?}: {e}", filelists_path)))?;
    }

    crate::output::step_success(format!("Initialised rpm repo at {:?}", base_path));
    crate::output::step_info("Structure: Packages/ → .rpm files, repodata/ → metadata");

    // Auto-add as a configured repository
    let repos = load_repos()?;
    if !repos.iter().any(|(n, _)| n == name) {
        let repo_url = format!("file://{}", base_path.to_string_lossy());
        add_repo(name, RepoSource::Rpm, Some(repo_url.clone()), None, None)?;
        crate::output::step_success(format!("Added '{}' to repository list (url={})", name, repo_url));
    }

    Ok(())
}

pub fn publish_package(name: &str, package_path: &str) -> SpmResult<()> {
    validate_repo_name(name)?;

    let repos = load_repos()?;
    let (_repo_name, config) = repos.iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| SpmError::config(format!("Repository '{name}' not found")))?;

    let url = config.url.as_deref().ok_or_else(|| {
        SpmError::config(format!("Repository '{name}' has no url configured"))
    })?;

    let base_path = url.strip_prefix("file://").ok_or_else(|| {
        SpmError::config(format!("Repository '{name}' url is not a local file:// path"))
    })?;
    let base = std::path::Path::new(base_path);

    let manifest = crate::package::sam::read_manifest(package_path)?;
    let sam_bytes = std::fs::read(package_path)
        .map_err(|e| SpmError::other(format!("Failed to read package: {e}")))?;
    let hash = crate::util::hash::hash_bytes(&sam_bytes);
    let filename = std::path::Path::new(package_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("package.sam")
        .to_string();

    match config.source {
        RepoSource::Native => publish_to_native(base, &manifest, &filename, &hash, &sam_bytes),
        RepoSource::Deb => publish_to_deb(base, config, &manifest, &filename, &hash, &sam_bytes),
        RepoSource::Rpm => publish_to_rpm(base, &manifest, &filename, &hash, &sam_bytes),
    }?;

    // Sign Release file if signing key is configured
    if let Some(key_path) = &config.signing_key {
        if std::path::Path::new(key_path).exists() {
            let _ = sign_after_publish(name, config, base);
        }
    }

    Ok(())
}

fn publish_to_native(
    base: &std::path::Path,
    manifest: &crate::types::Manifest,
    filename: &str,
    hash: &str,
    sam_bytes: &[u8],
) -> SpmResult<()> {
    let index_path = base.join("spm-repo.json");
    let content = std::fs::read_to_string(&index_path)
        .map_err(|e| SpmError::config(format!("Cannot read repo index: {e}")))?;
    let mut index: RepoIndex = serde_json::from_str(&content)
        .map_err(|e| SpmError::invalid_format(format!("Invalid repo index: {e}")))?;

    let pkg_path = base.join(filename);
    std::fs::write(&pkg_path, sam_bytes)
        .map_err(|e| SpmError::other(format!("Failed to write {:?}: {e}", pkg_path)))?;

    let deps: Vec<String> = manifest.dependencies.iter().map(|d| d.name.clone()).collect();
    let conflicts: Vec<String> = manifest.conflicts.to_vec();
    let provides_soname: Vec<String> = {
        let mut from_manifest: Vec<String> = manifest
            .provides
            .iter()
            .filter(|p| p.contains(".so"))
            .cloned()
            .collect();
        if from_manifest.is_empty() {
            // Fallback: scan the .sam file's ELF binaries for SONAMEs
            let scanned = crate::package::sam::scan_sam_sonames(
                &pkg_path.to_string_lossy()
            );
            if !scanned.is_empty() {
                from_manifest = scanned;
            }
        }
        from_manifest
    };
    index.packages.push(crate::types::RepoIndexRecord {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        architecture: manifest.architecture.clone(),
        description: manifest.description.clone(),
        dependencies: deps,
        provides_soname,
        conflicts,
        filename: filename.to_string(),
        hash: hash.to_string(),
        size: sam_bytes.len() as u64,
    });

    let json = serde_json::to_string_pretty(&index)
        .map_err(|e| SpmError::config(format!("Failed to serialize repo index: {e}")))?;
    std::fs::write(&index_path, &json)
        .map_err(|e| SpmError::config(format!("Cannot write repo index: {e}")))?;

    crate::output::step_success(format!("Published {} to native repo", filename));
    Ok(())
}

fn publish_to_deb(
    base: &std::path::Path,
    config: &RepoConfig,
    manifest: &crate::types::Manifest,
    filename: &str,
    _hash: &str,
    sam_bytes: &[u8],
) -> SpmResult<()> {
    let component = config.components.as_ref()
        .and_then(|c| c.first())
        .map(|s| s.as_str())
        .unwrap_or("main");
    let codename = config.codename.as_deref().unwrap_or("stable");

    let pool_dir = base.join("pool").join(component);
    std::fs::create_dir_all(&pool_dir)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", pool_dir)))?;

    let pkg_path = pool_dir.join(filename);
    std::fs::write(&pkg_path, sam_bytes)
        .map_err(|e| SpmError::other(format!("Failed to write {:?}: {e}", pkg_path)))?;

    let arch = &manifest.architecture;
    let dists_binary = base.join("dists").join(codename).join(component).join(format!("binary-{arch}"));

    let packages_path = dists_binary.join("Packages");
    let mut packages_content = String::new();
    if packages_path.exists() {
        packages_content = std::fs::read_to_string(&packages_path)
            .map_err(|e| SpmError::config(format!("Cannot read Packages: {e}")))?;
    }
    if !packages_content.ends_with('\n') {
        packages_content.push('\n');
    }

    let stanza = format!(
        "Package: {name}\nVersion: {version}\nArchitecture: {arch}\nMaintainer: {maintainer}\nDescription: {description}\nFilename: pool/{component}/{filename}\nSize: {size}\n\n",
        name = manifest.name,
        version = manifest.version,
        arch = arch,
        maintainer = manifest.maintainer,
        description = manifest.description,
        component = component,
        filename = filename,
        size = sam_bytes.len(),
    );
    packages_content.push_str(&stanza);

    std::fs::write(&packages_path, &packages_content)
        .map_err(|e| SpmError::config(format!("Cannot write Packages: {e}")))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(packages_content.as_bytes())
        .map_err(|e| SpmError::compression(e.to_string()))?;
    let compressed = encoder.finish()
        .map_err(|e| SpmError::compression(e.to_string()))?;
    let packages_gz_path = dists_binary.join("Packages.gz");
    std::fs::write(&packages_gz_path, &compressed)
        .map_err(|e| SpmError::config(format!("Cannot write Packages.gz: {e}")))?;

    crate::output::step_success(format!("Published {} to deb repo ({}/{})", filename, codename, component));
    Ok(())
}

fn publish_to_rpm(
    base: &std::path::Path,
    manifest: &crate::types::Manifest,
    filename: &str,
    _hash: &str,
    sam_bytes: &[u8],
) -> SpmResult<()> {
    let packages_dir = base.join("Packages");
    std::fs::create_dir_all(&packages_dir)
        .map_err(|e| SpmError::config(format!("Cannot create {:?}: {e}", packages_dir)))?;

    let pkg_path = packages_dir.join(filename);
    std::fs::write(&pkg_path, sam_bytes)
        .map_err(|e| SpmError::other(format!("Failed to write {:?}: {e}", pkg_path)))?;

    let repodata_dir = base.join("repodata");

    // Update primary.xml.gz
    let primary_path = repodata_dir.join("primary.xml.gz");
    let mut primary_content = String::new();
    if primary_path.exists() {
        use std::io::Read;
        let compressed = std::fs::read(&primary_path)
            .map_err(|e| SpmError::config(format!("Cannot read primary.xml.gz: {e}")))?;
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut text = String::new();
        decoder.read_to_string(&mut text)
            .map_err(|e| SpmError::compression(e.to_string()))?;
        primary_content = text;
    }

    if primary_content.contains("packages=\"") {
        let start = primary_content.find("packages=\"").unwrap() + 10;
        let end = primary_content[start..].find('"').unwrap() + start;
        let count: u32 = primary_content[start..end].parse().unwrap_or(0);
        let new_count = count + 1;
        primary_content = primary_content.replace(
            &format!("packages=\"{count}\""),
            &format!("packages=\"{new_count}\""),
        );
    }

    let pkg_xml = format!(
        r#"  <package type="sam">
    <name>{name}</name>
    <arch>{arch}</arch>
    <version epoch="0" ver="{version}" rel="1"/>
    <checksum type="sha256" pkgid="YES">{hash}</checksum>
    <summary>{description}</summary>
    <description>{description}</description>
    <packager>{maintainer}</packager>
    <size package="{size}" installed="{size}" archive="{size}"/>
    <location href="Packages/{filename}"/>
  </package>
"#,
        name = manifest.name,
        arch = manifest.architecture,
        version = manifest.version,
        hash = _hash,
        description = manifest.description.replace('&', "&amp;").replace('<', "&lt;"),
        maintainer = manifest.maintainer,
        size = sam_bytes.len(),
        filename = filename,
    );

    if let Some(pos) = primary_content.rfind("</metadata>") {
        primary_content.insert_str(pos, &pkg_xml);
    }

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(primary_content.as_bytes())
        .map_err(|e| SpmError::compression(e.to_string()))?;
    let compressed = encoder.finish()
        .map_err(|e| SpmError::compression(e.to_string()))?;
    std::fs::write(&primary_path, &compressed)
        .map_err(|e| SpmError::config(format!("Cannot write primary.xml.gz: {e}")))?;

    // Update filelists.xml.gz
    let filelists_path = repodata_dir.join("filelists.xml.gz");
    let mut filelists_content = String::new();
    if filelists_path.exists() {
        use std::io::Read;
        let compressed = std::fs::read(&filelists_path)
            .map_err(|e| SpmError::config(format!("Cannot read filelists.xml.gz: {e}")))?;
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut text = String::new();
        decoder.read_to_string(&mut text)
            .map_err(|e| SpmError::compression(e.to_string()))?;
        filelists_content = text;
    }

    if let Some(pos) = filelists_content.rfind("</filelists>") {
        let fl_xml = format!(
            r#"  <package pkgid="{hash}" name="{name}" arch="{arch}">
  </package>
"#,
            hash = _hash,
            name = manifest.name,
            arch = manifest.architecture,
        );
        filelists_content.insert_str(pos, &fl_xml);
    }

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(filelists_content.as_bytes())
        .map_err(|e| SpmError::compression(e.to_string()))?;
    let compressed = encoder.finish()
        .map_err(|e| SpmError::compression(e.to_string()))?;
    std::fs::write(&filelists_path, &compressed)
        .map_err(|e| SpmError::config(format!("Cannot write filelists.xml.gz: {e}")))?;

    crate::output::step_success(format!("Published {} to rpm repo", filename));
    Ok(())
}

pub fn remove_repo(name: &str) -> SpmResult<()> {
    validate_repo_name(name)?;
    let path = paths::repos_config_dir().join(format!("{}.list", name));
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| SpmError::config(format!("Cannot remove {:?}: {e}", path)))?;
    }
    // Clean up cached data for this repo
    for sub in &["deb", "rpm", "native"] {
        let cache_dir = paths::repos_cache_dir().join(sub).join(name);
        if cache_dir.exists() {
            let _ = fs::remove_dir_all(&cache_dir);
        }
    }
    // Also clean top-level cache (for dnf, legacy, or unknown types)
    let top_cache = paths::repos_cache_dir().join(name);
    if top_cache.exists() {
        let _ = fs::remove_dir_all(&top_cache);
    }
    Ok(())
}

pub fn list_repos() -> SpmResult<()> {
    let repos = load_repos()?;
    if repos.is_empty() {
        crate::output::step_info("No repositories configured.");
        crate::output::step_info("Add one: spm repo add <name> --source native --url <url>");
        return Ok(());
    }

    let max_name = repos.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    let col_w = max_name.max(4);
    println!("  {} {:<cw$}  {:7}  URL",
        crate::output::cyan("▸"), "NAME", "TYPE",
        cw = col_w);
    println!("  {}",
        crate::output::dim(std::iter::repeat_n('─', col_w + 30).collect::<String>()));
    for (name, config) in &repos {
        let url = config.url.as_deref().unwrap_or("-");
        println!("  {:<cw$}  {:7}  {}",
            crate::output::bold(name),
            crate::output::green(config.source.to_string()),
            url,
            cw = col_w);
    }
    Ok(())
}

pub fn update_repos() -> SpmResult<()> {
    // Bump dependency cache epoch so stale entries are discarded
    crate::package::resolver::bump_dep_cache_epoch();

    let repos = load_repos()?;
    let mut updated = 0u32;
    for (name, config) in &repos {
        let result = match config.source {
            RepoSource::Deb => update_from_deb(name, config),
            RepoSource::Rpm => update_from_rpm(name, config),
            RepoSource::Native => update_from_native(name, config),
        };
        match result {
            Ok(()) => {
                crate::output::step_success(format!("Updated '{name}' ({})", config.source));
                updated += 1;
            }
            Err(e) => {
                crate::output::step_warn(format!("Failed to update '{name}': {e}"));
            }
        }
    }
    crate::output::result_message(format!("Updated {updated}/{} repositories", repos.len()));

    // Build SONAME index from cached repo metadata
    crate::output::step_info("Building SONAME index...");
    if let Err(e) = crate::index::build_index() {
        crate::output::step_warn(format!("SONAME index build failed: {e}"));
    }

    Ok(())
}

fn update_from_deb(name: &str, config: &RepoConfig) -> SpmResult<()> {
    let cache = paths::repos_cache_dir().join("deb").join(name);
    let sources_file = PathBuf::from("/etc/apt/sources.list");
    let sources_d = PathBuf::from("/etc/apt/sources.list.d");

    fs::create_dir_all(&cache)
        .map_err(|e| SpmError::config(format!("Cannot create deb cache: {e}")))?;

    let mut found = false;

    // Copy system apt sources if available (backward compat)
    if sources_file.exists() {
        if let Ok(content) = fs::read_to_string(&sources_file) {
            fs::write(cache.join("sources.list"), &content).ok();
            found = true;
        }
    }
    if sources_d.exists() {
        if let Ok(dir) = fs::read_dir(&sources_d) {
            for entry in dir.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "list" || ext == "sources" {
                        if let Ok(content) = fs::read_to_string(&path) {
                            fs::write(cache.join(path.file_name().unwrap_or_default()), &content).ok();
                            found = true;
                        }
                    }
                }
            }
        }
    }

    // Download Packages.gz from mirrors if configured (HTTP-based apt)
    if let (Some(mirrors), Some(codename), Some(components)) =
        (&config.mirrors, &config.codename, &config.components)
    {
        let arch = "amd64";
        for mirror in mirrors {
            let base = mirror.trim_end_matches('/');
            for component in components {
                let packages_url = format!(
                    "{base}/dists/{codename}/{component}/binary-{arch}/Packages.gz"
                );
                let cache_name = format!("Packages-{codename}-{component}-{arch}");
                let cache_path = cache.join(&cache_name);

                match fetch_with_retry(&packages_url, 2) {
                    Ok(compressed) => {
                        // Decompress gz
                        use std::io::Read;
                        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
                        let mut text = Vec::new();
                        if decoder.read_to_end(&mut text).is_ok()
                            && fs::write(&cache_path, &text).is_ok() {
                                crate::output::step_success(format!(
                                    "Cached '{component}/binary-{arch}' from {mirror}"
                                ));
                                found = true;
                            }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch Packages.gz from {mirror}: {e}");
                    }
                }
            }
        }
    }

    if !found {
        tracing::warn!("No deb sources found for '{name}'");
    }
    Ok(())
}

fn update_from_rpm(name: &str, _config: &RepoConfig) -> SpmResult<()> {
    let cache = paths::repos_cache_dir().join("rpm").join(name);
    let repos_d = PathBuf::from("/etc/yum.repos.d");

    fs::create_dir_all(&cache)
        .map_err(|e| SpmError::config(format!("Cannot create rpm cache: {e}")))?;

    let mut found = false;
    if repos_d.exists() {
        if let Ok(dir) = fs::read_dir(&repos_d) {
            for entry in dir.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "repo" {
                        if let Ok(content) = fs::read_to_string(&path) {
                            let fname = path.file_name().unwrap_or_default();
                            fs::write(cache.join(fname), &content).ok();
                            found = true;
                        }
                    }
                }
            }
        }
    }

    if !found {
        tracing::warn!("No rpm/yum repos found on this system");
    }
    Ok(())
}

fn update_from_native(name: &str, config: &RepoConfig) -> SpmResult<()> {
    let url = config.url.as_deref().ok_or_else(|| {
        SpmError::config(format!("Native repo '{name}' has no url configured"))
    })?;

    let base = url.trim_end_matches('/');
    let index_url = format!("{base}/spm-repo.json");
    let cache_dir = paths::repos_cache_dir().join("native").join(name);

    fs::create_dir_all(&cache_dir)
        .map_err(|e| SpmError::config(format!("Cannot create cache dir: {e}")))?;

    let body = fetch_with_retry(&index_url, 3)?;

    let index: RepoIndex = serde_json::from_slice(&body)
        .map_err(|e| SpmError::invalid_format(format!("Invalid repo index from {index_url}: {e}")))?;

    if index.format_version != 1 {
        tracing::warn!("Repo '{name}' uses format version {}, expected 1", index.format_version);
    }

    let index_path = cache_dir.join("repo-index.json");
    fs::write(&index_path, &body)
        .map_err(|e| SpmError::config(format!("Cannot write cached index: {e}")))?;

    crate::output::step_success(format!("Fetched {} packages from '{}'", index.packages.len(), index.repo_name));
    Ok(())
}

pub fn detect_source() -> RepoSource {
    let os_release = std::path::Path::new("/etc/os-release");
    if let Ok(content) = fs::read_to_string(os_release) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("ID=") {
                let id = val.trim_matches('"').to_lowercase();
                if id == "debian" || id == "ubuntu" || id == "linuxmint" || id == "kali" {
                    return RepoSource::Deb;
                }
                if id == "fedora" || id == "rhel" || id == "centos" || id == "rocky" || id == "almalinux" {
                    return RepoSource::Rpm;
                }
                if id == "opensuse-leap" || id == "opensuse-tumbleweed" || id == "suse" {
                    return RepoSource::Rpm;
                }
            }
        }
    }
    RepoSource::Native
}

fn validate_repo_name(name: &str) -> SpmResult<()> {
    if name.contains('/') || name.contains("..") || name.contains('\0') {
        return Err(SpmError::config(format!(
            "Invalid repository name '{}': must not contain '/', '..', or null bytes", name
        )));
    }
    Ok(())
}

/// Generate an Ed25519 signing keypair for a repository.
/// Saves the private key as PEM and stores the path in the repo config.
pub fn generate_signing_key(repo_name: &str) -> SpmResult<()> {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    use rand::rngs::OsRng;

    let mut repos = load_repos()?;
    let pos = repos.iter().position(|(n, _)| n == repo_name)
        .ok_or_else(|| SpmError::config(format!("Repository '{repo_name}' not found")))?;

    let key_dir = paths::repos_config_dir().join(SIGNING_DIR_NAME);
    fs::create_dir_all(&key_dir)
        .map_err(|e| SpmError::config(format!("Cannot create key dir: {e}")))?;

    let secret_path = key_dir.join(format!("{repo_name}-key.pem"));
    let public_path = key_dir.join(format!("{repo_name}-key.pub"));

    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    let signing_key = SigningKey::from_bytes(&secret);
    let verify_key = signing_key.verifying_key();

    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let pem = signing_key.to_pkcs8_pem(Default::default())
        .map_err(|e| SpmError::other(format!("Failed to encode private key: {e}")))?;
    fs::write(&secret_path, pem.as_str())
        .map_err(|e| SpmError::config(format!("Cannot write private key: {e}")))?;

    use ed25519_dalek::pkcs8::EncodePublicKey;
    let pub_pem = verify_key.to_public_key_pem(Default::default())
        .map_err(|e| SpmError::other(format!("Failed to encode public key: {e}")))?;
    fs::write(&public_path, &pub_pem)
        .map_err(|e| SpmError::config(format!("Cannot write public key: {e}")))?;

    // Update repo config with signing key path
    let (_name, ref mut config) = &mut repos[pos];
    config.signing_key = Some(secret_path.to_string_lossy().to_string());

    let config_path = paths::repos_config_dir().join(format!("{repo_name}.list"));
    let toml_str = toml::to_string(&config)
        .map_err(|e| SpmError::config(format!("Failed to serialize repo config: {e}")))?;
    fs::write(&config_path, &toml_str)
        .map_err(|e| SpmError::config(format!("Cannot write repo config: {e}")))?;

    crate::output::step_success(format!("Generated Ed25519 signing key for '{repo_name}'"));
    crate::output::step_info(format!("Private key: {:?}", secret_path));
    crate::output::step_info(format!("Public key:  {:?}", public_path));
    Ok(())
}

/// Sign a repository's Release file with its Ed25519 key.
pub fn sign_repo(repo_name: &str) -> SpmResult<()> {
    let repos = load_repos()?;
    let (_name, config) = repos.iter()
        .find(|(n, _)| n == repo_name)
        .ok_or_else(|| SpmError::config(format!("Repository '{repo_name}' not found")))?;

    let signing_key_path = config.signing_key.as_deref()
        .ok_or_else(|| SpmError::config(format!("Repository '{repo_name}' has no signing key configured. Use `spm repo gen-key {repo_name}` first.")))?;

    let url = config.url.as_deref().ok_or_else(|| {
        SpmError::config(format!("Repository '{repo_name}' has no url configured"))
    })?;
    let base_path = url.strip_prefix("file://").ok_or_else(|| {
        SpmError::config(format!("Repository '{repo_name}' url is not a local file:// path"))
    })?;
    let base = std::path::Path::new(base_path);

    // Read private key
    let pem_str = fs::read_to_string(signing_key_path)
        .map_err(|e| SpmError::config(format!("Cannot read signing key: {e}")))?;
    use ed25519_dalek::pkcs8::DecodePrivateKey;
    let signing_key = ed25519_dalek::SigningKey::from_pkcs8_pem(&pem_str)
        .map_err(|e| SpmError::other(format!("Invalid signing key: {e}")))?;

    match config.source {
        RepoSource::Native => sign_native_repo(base, &signing_key)?,
        RepoSource::Deb => sign_deb_repo(base, &signing_key)?,
        RepoSource::Rpm => sign_rpm_repo(base, &signing_key)?,
    }

    crate::output::step_success(format!("Signed Release file for '{repo_name}'"));
    Ok(())
}

fn sign_native_repo(base: &std::path::Path, signing_key: &ed25519_dalek::SigningKey) -> SpmResult<()> {
    use base64::Engine;
    use ed25519_dalek::Signer;

    let index_path = base.join("spm-repo.json");
    if !index_path.exists() {
        return Err(SpmError::config("spm-repo.json not found. Run `spm repo publish` first."));
    }
    let index_bytes = fs::read(&index_path)
        .map_err(|e| SpmError::config(format!("Cannot read spm-repo.json: {e}")))?;
    let index_hash = crate::util::hash::hash_bytes(&index_bytes);

    let release_content = format!(
        "Repository-Format: native\nCreated: {}\nMetadata: spm-repo.json\nMetadata-SHA256: {}\nMetadata-Size: {}\n",
        chrono::Utc::now().to_rfc3339(),
        index_hash,
        index_bytes.len(),
    );
    let release_path = base.join("Release");
    fs::write(&release_path, &release_content)
        .map_err(|e| SpmError::config(format!("Cannot write Release: {e}")))?;

    let sig = signing_key.sign(release_content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let sig_path = base.join("Release.sig");
    fs::write(&sig_path, &sig_b64)
        .map_err(|e| SpmError::config(format!("Cannot write Release.sig: {e}")))?;

    Ok(())
}

fn sign_deb_repo(base: &std::path::Path, signing_key: &ed25519_dalek::SigningKey) -> SpmResult<()> {
    use base64::Engine;
    use ed25519_dalek::Signer;

    // For apt repos, we need to find all Packages files under dists/ and generate Release
    let dists_dir = base.join("dists");
    if !dists_dir.exists() {
        return Err(SpmError::config("dists/ directory not found. Create the repo first."));
    }

    let mut releases = Vec::new();
    collect_release_entries(&dists_dir, &dists_dir, &mut releases)?;

    let release_content = format!(
        "Origin: spm\nLabel: {}\nSuite: {}\nCodename: {}\nDate: {}\nArchitectures: amd64\nComponents: main\nDescription: spm apt repository\n{}",
        dists_dir.file_name().unwrap_or_default().to_string_lossy(),
        "stable",
        "stable",
        chrono::Utc::now().to_rfc3339(),
        releases.join(""),
    );

    // Write Release at dists/<codename>/Release
    let release_path = dists_dir.join("Release");
    fs::write(&release_path, &release_content)
        .map_err(|e| SpmError::config(format!("Cannot write Release: {e}")))?;

    let sig = signing_key.sign(release_content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let sig_path = dists_dir.join("Release.sig");
    fs::write(&sig_path, &sig_b64)
        .map_err(|e| SpmError::config(format!("Cannot write Release.sig: {e}")))?;

    // Write InRelease (OpenPGP-clearsigned Release)
    let inrelease_path = dists_dir.join("InRelease");
    let inrelease_data = create_inrelease(&release_content, signing_key)?;
    fs::write(&inrelease_path, &inrelease_data)
        .map_err(|e| SpmError::config(format!("Cannot write InRelease: {e}")))?;

    Ok(())
}

/// Create an InRelease file: the Release content followed by an Ed25519 signature.
/// This replaces the previous gpg --clearsign dependency.
fn create_inrelease(content: &str, signing_key: &ed25519_dalek::SigningKey) -> SpmResult<Vec<u8>> {
    use base64::Engine;
    use ed25519_dalek::Signer;

    let sig = signing_key.sign(content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let inrelease = format!("{content}\n-----BEGIN SIGNATURE-----\n{sig_b64}\n-----END SIGNATURE-----\n");
    Ok(inrelease.into_bytes())
}

fn collect_release_entries(root: &std::path::Path, dir: &std::path::Path, entries: &mut Vec<String>) -> SpmResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| SpmError::config(format!("Cannot read dir: {e}")))? {
        let entry = entry.map_err(|e| SpmError::config(format!("Dir entry error: {e}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_release_entries(root, &path, entries)?;
        } else if path.is_file() {
            // Include metadata files (Packages, Packages.gz, etc.)
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname == "Release" || fname == "Release.gpg" || fname == "InRelease" {
                continue;
            }
            let rel_path = path.strip_prefix(root)
                .map_err(|_| SpmError::config("Path error"))?;
            let rel_str = rel_path.to_string_lossy();
            let bytes = fs::read(&path)
                .map_err(|e| SpmError::config(format!("Cannot read {:?}: {e}", path)))?;
            let hash = crate::util::hash::hash_bytes(&bytes);
            entries.push(format!(" {} {} {}\n", hash, bytes.len(), rel_str));
        }
    }
    Ok(())
}

fn sign_rpm_repo(base: &std::path::Path, signing_key: &ed25519_dalek::SigningKey) -> SpmResult<()> {
    use base64::Engine;
    use ed25519_dalek::Signer;

    let repodata_dir = base.join("repodata");
    if !repodata_dir.exists() {
        return Err(SpmError::config("repodata/ directory not found."));
    }

    // Generate repomd.xml with checksums
    let mut repomd_content = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd xmlns="http://linux.duke.edu/metadata/repo">
"#);

    for entry in fs::read_dir(&repodata_dir).map_err(|e| SpmError::config(format!("Cannot read repodata: {e}")))? {
        let entry = entry.map_err(|e| SpmError::config(format!("Dir entry error: {e}")))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname == "repomd.xml" || fname == "repomd.xml.asc" {
            continue;
        }
        let data_type = fname.split('.').next().unwrap_or("unknown");
        let bytes = fs::read(&path)
            .map_err(|e| SpmError::config(format!("Cannot read {:?}: {e}", path)))?;
        let hash = crate::util::hash::hash_bytes(&bytes);
        let size = bytes.len();
        repomd_content.push_str(&format!(
            r#"  <data type="{data_type}">
    <checksum type="sha256">{hash}</checksum>
    <open-checksum type="sha256">{hash}</open-checksum>
    <location href="repodata/{fname}"/>
    <size>{size}</size>
  </data>
"#));
    }

    repomd_content.push_str("</repomd>\n");

    let repomd_path = repodata_dir.join("repomd.xml");
    fs::write(&repomd_path, &repomd_content)
        .map_err(|e| SpmError::config(format!("Cannot write repomd.xml: {e}")))?;

    let sig = signing_key.sign(repomd_content.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    let sig_path = repodata_dir.join("repomd.xml.asc");
    fs::write(&sig_path, &sig_b64)
        .map_err(|e| SpmError::config(format!("Cannot write repomd.xml.asc: {e}")))?;

    Ok(())
}

/// Generate and sign a Release file after publishing a package.
fn sign_after_publish(_name: &str, config: &RepoConfig, base: &std::path::Path) -> SpmResult<()> {
    if let Some(key_path) = &config.signing_key {
        if !std::path::Path::new(key_path).exists() {
            crate::output::step_warn(format!("Signing key not found at {key_path}, skipping signing"));
            return Ok(());
        }
        let pem_str = fs::read_to_string(key_path)
            .map_err(|e| SpmError::config(format!("Cannot read signing key: {e}")))?;
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        let signing_key = ed25519_dalek::SigningKey::from_pkcs8_pem(&pem_str)
            .map_err(|e| SpmError::other(format!("Invalid signing key: {e}")))?;

        match config.source {
            RepoSource::Native => sign_native_repo(base, &signing_key)?,
            RepoSource::Deb => sign_deb_repo(base, &signing_key)?,
            RepoSource::Rpm => sign_rpm_repo(base, &signing_key)?,
        }
        crate::output::step_success("Signed Release file");
    }
    Ok(())
}

pub(crate) fn fetch_with_retry(url: &str, max_retries: u32) -> SpmResult<Vec<u8>> {
    let mut last_error = None;
    for attempt in 1..=max_retries {
        match ureq::get(url).call() {
            Ok(mut response) => {
                match response.body_mut().with_config().limit(256 * 1024 * 1024).read_to_vec() {
                    Ok(body) => return Ok(body),
                    Err(e) => {
                        last_error = Some(SpmError::network(format!("Read error: {e}")));
                    }
                }
            }
            Err(e) => {
                last_error = Some(SpmError::network(format!("Attempt {attempt}/{max_retries}: {e}")));
                if attempt < max_retries {
                    std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| SpmError::network("Fetch failed after retries".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SpmError;

    #[test]
    fn test_validate_repo_name_valid() {
        assert!(validate_repo_name("my-repo").is_ok());
        assert!(validate_repo_name("fedora-40").is_ok());
        assert!(validate_repo_name("debian_bookworm").is_ok());
    }

    #[test]
    fn test_validate_repo_name_with_slash() {
        let err = validate_repo_name("repo/name").unwrap_err();
        assert!(format!("{}", err).contains("Invalid repository name"));
    }

    #[test]
    fn test_validate_repo_name_with_dotdot() {
        let err = validate_repo_name("..").unwrap_err();
        assert!(format!("{}", err).contains("Invalid repository name"));
    }

    #[test]
    fn test_validate_repo_name_with_null() {
        let err = validate_repo_name("bad\0name").unwrap_err();
        assert!(format!("{}", err).contains("Invalid repository name"));
    }

    #[test]
    fn test_detect_source_fallback() {
        // When /etc/os-release doesn't exist, should return Native
        let source = detect_source();
        // Can't easily test specific ID without mocking, but at least doesn't crash
        let _ = source;
    }

    #[test]
    fn test_add_repo_validates_name() {
        let err = add_repo("with/slash", RepoSource::Native, None, None, None).unwrap_err();
        assert!(matches!(err, SpmError::Config(_)));
    }

    #[test]
    fn test_remove_repo_validates_name() {
        let err = remove_repo("with/slash").unwrap_err();
        assert!(matches!(err, SpmError::Config(_)));
    }

    #[test]
    fn test_create_repo_validates_name() {
        let err = create_repo("with/slash", RepoSource::Native, None, None, None, None).unwrap_err();
        assert!(matches!(err, SpmError::Config(_)));
    }
}
