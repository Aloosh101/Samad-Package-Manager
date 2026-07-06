use std::collections::HashMap;
use std::io::Read;

use crate::error::{SpmError, SpmResult};

pub fn parse_deb822(content: &str) -> Vec<HashMap<String, String>> {
    let mut entries = Vec::new();
    let mut current_entry = HashMap::new();
    let mut current_key = String::new();
    let mut current_value = String::new();

    for line in content.lines() {
        if line.is_empty() {
            if !current_key.is_empty() {
                current_entry.insert(current_key.clone(), current_value.trim().to_string());
                current_key.clear();
                current_value.clear();
            }
            if !current_entry.is_empty() {
                entries.push(std::mem::take(&mut current_entry));
            }
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            if !current_key.is_empty() {
                current_value.push('\n');
                current_value.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            if !current_key.is_empty() {
                current_entry.insert(current_key.clone(), current_value.trim().to_string());
            }
            current_key = k.trim().to_string();
            current_value = v.trim().to_string();
        }
    }

    if !current_key.is_empty() {
        current_entry.insert(current_key, current_value.trim().to_string());
    }
    if !current_entry.is_empty() {
        entries.push(current_entry);
    }

    entries
}

pub fn fetch_debian_package(
    name: &str,
    version: Option<&str>,
    repo_url: &str,
    distro: &str,
    component: &str,
) -> SpmResult<Vec<u8>> {
    let base = repo_url.trim_end_matches('/');
    let arch = host_arch();

    let packages_url = format!(
        "{base}/dists/{distro}/{component}/binary-{arch}/Packages.gz"
    );

    let compressed = download_bytes(&packages_url)?;

    let text = decompress_packages(compressed)?;

    let entries = parse_deb822(&text);
    let entry = entries
        .iter()
        .find(|e| {
            e.get("Package")
                .map_or(false, |p| p.eq_ignore_ascii_case(name))
                && version.map_or(true, |v| {
                    e.get("Version").map_or(false, |ev| ev == v)
                })
        })
        .ok_or_else(|| {
            SpmError::package_not_found(format!(
                "Package '{name}' not found in {base}/dists/{distro} {component}"
            ))
        })?;

    let filename = entry.get("Filename").ok_or_else(|| {
        SpmError::invalid_format(format!(
            "Package entry for '{name}' has no Filename field"
        ))
    })?;

    let expected_sha256 = entry.get("SHA256").and_then(|s| {
        s.lines().find_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && *parts[2] == *filename {
                Some(parts[0].to_string())
            } else {
                None
            }
        })
    });

    let deb_url = format!("{base}/{filename}");
    let body = download_bytes(&deb_url)?;

    if let Some(ref expected) = expected_sha256 {
        let actual = crate::util::hash::sha256_hex(&body);
        if actual != *expected {
            return Err(SpmError::other(format!(
                "SHA256 mismatch for '{name}': expected {expected}, got {actual}"
            )));
        }
    }

    Ok(body)
}

fn download_bytes(url: &str) -> SpmResult<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| SpmError::network(format!("Failed to create HTTP client: {e}")))?;
    let response = client
        .get(url)
        .send()
        .map_err(|e| SpmError::network(format!("Failed to fetch {url}: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(SpmError::network(format!("HTTP {status} fetching {url}")));
    }
    response
        .bytes()
        .map(|b| b.to_vec())
        .map_err(|e| SpmError::network(format!("Failed to read response body from {url}: {e}")))
}

fn decompress_packages(data: Vec<u8>) -> SpmResult<String> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(&data[..]);
        let mut text = String::new();
        decoder
            .read_to_string(&mut text)
            .map_err(|e| SpmError::compression(format!("Failed to decompress gz: {e}")))?;
        Ok(text)
    } else if data.len() > 4 && data.starts_with(b"\xfd7zXZ") {
        let mut decoder = xz2::read::XzDecoder::new(&data[..]);
        let mut text = String::new();
        decoder
            .read_to_string(&mut text)
            .map_err(|e| SpmError::compression(format!("Failed to decompress xz: {e}")))?;
        Ok(text)
    } else {
        String::from_utf8(data)
            .map_err(|e| SpmError::invalid_format(format!("Invalid UTF-8 in Packages file: {e}")))
    }
}

fn host_arch() -> String {
    let arch = std::env::consts::ARCH;
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        other => other,
    }
    .to_string()
}
