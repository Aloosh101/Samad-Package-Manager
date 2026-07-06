use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{SpmError, SpmResult};
use crate::types::{Manifest, SysuserEntry, TmpfileEntry, Trigger};

#[derive(Debug, Deserialize)]
struct BuildConfig {
    package: PackageConfig,
    #[serde(default)]
    build: BuildSection,
    #[serde(default)]
    install: InstallSection,
}

#[derive(Debug, Deserialize)]
struct PackageConfig {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_arch")]
    architecture: String,
    #[serde(default = "default_maintainer")]
    maintainer: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    provides: Vec<String>,
    // SAM v2 fields
    #[serde(default)]
    systemd_units: Vec<String>,
    #[serde(default)]
    sysusers: Vec<SysuserEntry>,
    #[serde(default)]
    tmpfiles: Vec<TmpfileEntry>,
    #[serde(default)]
    triggers: Vec<Trigger>,
    #[serde(default)]
    obsoletes: Vec<String>,
    #[serde(default)]
    conffiles: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct BuildSection {
    command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InstallSection {
    #[serde(default)]
    files: Vec<String>,
    dir: Option<String>,
    #[serde(default)]
    scripts_dir: Option<String>,
}

fn default_arch() -> String {
    std::env::consts::ARCH.to_string()
}

fn default_maintainer() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let host = hostname();
    format!("{user}@{host}")
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn find_config_dir(path: &Path) -> SpmResult<PathBuf> {
    let config_path = path.join("spm-build.toml");
    if config_path.exists() {
        return Ok(path.to_path_buf());
    }
    Err(SpmError::config(format!(
        "No spm-build.toml found in {}",
        path.display()
    )))
}

fn load_config(path: &Path) -> SpmResult<BuildConfig> {
    let content = fs::read_to_string(path.join("spm-build.toml"))
        .map_err(|e| SpmError::config(format!("Failed to read spm-build.toml: {e}")))?;
    toml::from_str(&content)
        .map_err(|e| SpmError::config(format!("Failed to parse spm-build.toml: {e}")))
}

fn convert_deps(raw: &[String]) -> Vec<crate::types::Dependency> {
    raw.iter().map(|d| crate::types::Dependency {
        name: d.clone(),
        version: String::new(),
        source: crate::types::DependencySource::System,
        format: Some(crate::types::PackageFormat::Sam),
    }).collect()
}

fn create_tar_from_files(root: &Path, files: &[String], install_dir: &str) -> SpmResult<String> {
    let tar_path = root.join(".spm-build-data.tar");
    let file = fs::File::create(&tar_path)
        .map_err(|e| SpmError::other(format!("Failed to create tar: {e}")))?;
    let mut builder = tar::Builder::new(file);

    for entry in files {
        let (src, dst) = entry.split_once(':').unwrap_or((entry, entry));
        let src_path = root.join(src);
        let dst_path = Path::new(install_dir).join(dst);

        if !src_path.exists() {
            return Err(SpmError::config(format!(
                "Source file '{}' specified in install.files not found",
                src
            )));
        }

        if src_path.is_dir() {
            builder.append_dir_all(&dst_path, &src_path)
                .map_err(|e| SpmError::other(format!("Failed to add directory {src}: {e}")))?;
        } else {
            builder.append_path_with_name(&src_path, &dst_path)
                .map_err(|e| SpmError::other(format!("Failed to add file {src}: {e}")))?;
        }
    }

    builder.finish()
        .map_err(|e| SpmError::other(format!("Failed to finish tar: {e}")))?;

    Ok(tar_path.to_string_lossy().to_string())
}

fn create_tar_from_dir(root: &Path, dir: &str, install_dir: &str) -> SpmResult<String> {
    let src_dir = root.join(dir);
    if !src_dir.is_dir() {
        return Err(SpmError::config(format!(
            "Install directory '{}' not found or not a directory",
            dir
        )));
    }

    let tar_path = root.join(".spm-build-data.tar");
    let file = fs::File::create(&tar_path)
        .map_err(|e| SpmError::other(format!("Failed to create tar: {e}")))?;
    let mut builder = tar::Builder::new(file);

    builder.append_dir_all(install_dir, &src_dir)
        .map_err(|e| SpmError::other(format!("Failed to archive directory {dir}: {e}")))?;

    builder.finish()
        .map_err(|e| SpmError::other(format!("Failed to finish tar: {e}")))?;

    Ok(tar_path.to_string_lossy().to_string())
}

fn find_scripts_dir(root: &Path, config: &InstallSection) -> Option<String> {
    let scripts_dir = config.scripts_dir.as_deref().unwrap_or("scripts");
    let scripts_path = root.join(scripts_dir);
    if scripts_path.is_dir() {
        let tar_path = root.join(".spm-build-meta.tar");
        match create_scripts_tar(&scripts_path, &tar_path) {
            Ok(()) => Some(tar_path.to_string_lossy().to_string()),
            Err(_) => None,
        }
    } else {
        None
    }
}

fn create_scripts_tar(scripts_dir: &Path, output: &Path) -> SpmResult<()> {
    let file = fs::File::create(output)
        .map_err(|e| SpmError::other(format!("Failed to create scripts tar: {e}")))?;
    let mut builder = tar::Builder::new(file);

    for entry in fs::read_dir(scripts_dir)
        .map_err(|e| SpmError::other(format!("Failed to read scripts dir: {e}")))?
    {
        let entry = entry.map_err(|e| SpmError::other(format!("Dir entry error: {e}")))?;
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            builder.append_path_with_name(&path, &name)
                .map_err(|e| SpmError::other(format!("Failed to add script {name}: {e}")))?;
        }
    }

    builder.finish()
        .map_err(|e| SpmError::other(format!("Failed to finish scripts tar: {e}")))?;
    Ok(())
}

fn calculate_dir_size(root: &Path, files: &[String], _install_dir: &str) -> SpmResult<u64> {
    let mut total = 0u64;

    for entry in files {
        let (src, _dst) = entry.split_once(':').unwrap_or((entry, entry));
        let src_path = root.join(src);
        if src_path.is_dir() {
            total += dir_size(&src_path)?;
        } else if src_path.is_file() {
            total += src_path.metadata()
                .map(|m| m.len())
                .unwrap_or(0);
        }
    }

    Ok(total)
}

fn dir_size(path: &Path) -> SpmResult<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|e| SpmError::other(format!("Failed to read dir {}: {e}", path.display())))?
        {
            let entry = entry
                .map_err(|e| SpmError::other(format!("Dir entry error: {e}")))?;
            let path = entry.path();
            if path.is_dir() {
                total += dir_size(&path)?;
            } else if path.is_file() {
                total += entry.metadata()
                    .map(|m| m.len())
                    .unwrap_or(0);
            }
        }
    }
    Ok(total)
}

pub fn build_package(path: &Path, output: Option<&str>, sign_key: Option<&str>) -> SpmResult<String> {
    let root = find_config_dir(path)?;
    let config = load_config(&root)?;

    let install_dir = "";

    let data_tar = if let Some(dir) = &config.install.dir {
        create_tar_from_dir(&root, dir, install_dir)?
    } else {
        create_tar_from_files(&root, &config.install.files, install_dir)?
    };

    let meta_tar = find_scripts_dir(&root, &config.install);

    let size = if let Some(dir) = &config.install.dir {
        dir_size(&root.join(dir))?
    } else {
        calculate_dir_size(&root, &config.install.files, install_dir)?
    };

    let pkg_name = if sign_key.is_some() {
        format!("{}-{}.signed.sam", config.package.name, config.package.version)
    } else {
        format!("{}-{}.sam", config.package.name, config.package.version)
    };

    let output_path = output
        .map(|o| PathBuf::from(o).join(&pkg_name))
        .unwrap_or_else(|| root.join(&pkg_name));

    let manifest = Manifest {
        name: config.package.name.clone(),
        version: config.package.version.clone(),
        architecture: config.package.architecture.clone(),
        maintainer: config.package.maintainer.clone(),
        description: config.package.description.clone(),
        dependencies: convert_deps(&config.package.dependencies),
        conflicts: config.package.conflicts.clone(),
        provides: config.package.provides.clone(),
        recommends: vec![],
        install_size: size,
        format_version: 1,
        source: None,
        ai_metadata: None,
        signature: None,
        systemd_units: config.package.systemd_units.clone(),
        sysusers: config.package.sysusers.clone(),
        tmpfiles: config.package.tmpfiles.clone(),
        triggers: config.package.triggers.clone(),
        obsoletes: config.package.obsoletes.clone(),
        conffiles: config.package.conffiles.clone(),
    };

    crate::package::sam::create_sam(
        &manifest,
        &data_tar,
        meta_tar.as_deref(),
        &output_path.to_string_lossy(),
    )?;

    let _ = fs::remove_file(&data_tar);
    if let Some(ref meta) = meta_tar {
        let _ = fs::remove_file(meta);
    }

    Ok(output_path.to_string_lossy().to_string())
}

pub fn run_build(path: &Path, output: Option<&str>, sign_key: Option<&str>) -> SpmResult<String> {
    let root = find_config_dir(path)?;
    let config = load_config(&root)?;

    if let Some(ref cmd) = config.build.command {
        crate::output::step_warn(format!(
            "Running build command as root: $ {cmd}"
        ));
        let status = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(&root)
            .status()
            .map_err(|e| SpmError::command_failed(format!("Failed to run build command: {e}")))?;
        if !status.success() {
            return Err(SpmError::command_failed(
                "Build command exited with non-zero status",
            ));
        }
    }

    let output_path = build_package(path, output, sign_key)?;
    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_deps_empty() {
        let result = convert_deps(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_deps_single() {
        let result = convert_deps(&["libfoo".to_string()]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "libfoo");
        assert_eq!(result[0].format, Some(crate::types::PackageFormat::Sam));
    }

    #[test]
    fn test_convert_deps_multiple() {
        let result = convert_deps(&["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "a");
        assert_eq!(result[1].name, "b");
        assert_eq!(result[2].name, "c");
    }

    #[test]
    fn test_default_arch_not_empty() {
        let arch = default_arch();
        assert!(!arch.is_empty());
    }
}
