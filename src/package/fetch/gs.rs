use std::io::Read;

use crate::error::{SpmError, SpmResult};

/// Parse a repomd.xml file and return the primary.xml.gz location + checksum.
/// repomd.xml is the RPM-MD repository metadata index.
pub(crate) fn parse_repomd(body: &str) -> SpmResult<(String, String, String)> {
    let mut in_data = false;
    let mut current_type = String::new();
    let mut current_location = String::new();
    let mut current_checksum = String::new();
    let mut current_cs_type = String::new();
    let mut in_checksum = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<data type=") {
            in_data = true;
            current_type = extract_attr(trimmed, "type");
            continue;
        }
        if trimmed.starts_with("</data>") {
            if in_data && current_type == "primary" {
                return Ok((
                    std::mem::take(&mut current_location),
                    std::mem::take(&mut current_checksum),
                    std::mem::take(&mut current_cs_type),
                ));
            }
            in_data = false;
            current_type.clear();
            current_location.clear();
            current_checksum.clear();
            current_cs_type.clear();
            continue;
        }
        if !in_data {
            continue;
        }
        if trimmed.starts_with("<location") {
            current_location = extract_attr(trimmed, "href");
            continue;
        }
        if trimmed.starts_with("<checksum") {
            in_checksum = true;
            current_cs_type = extract_attr(trimmed, "type");
            if let Some(text_start) = trimmed.find('>') {
                let after_tag = &trimmed[text_start + 1..];
                if let Some(text_end) = after_tag.find('<') {
                    current_checksum = after_tag[..text_end].to_string();
                } else if !after_tag.is_empty() {
                    current_checksum.push_str(after_tag);
                }
            }
            continue;
        }
        if trimmed.starts_with("</checksum>") {
            in_checksum = false;
            continue;
        }
        if in_checksum && !trimmed.starts_with('<') {
            current_checksum.push_str(trimmed);
        }
    }

    Err(SpmError::other("primary.xml.gz not found in repomd.xml".to_string()))
}

/// Package location info extracted from primary.xml
pub(crate) struct PrimaryInfo {
    pub location: String,
    pub checksum: String,
    #[allow(dead_code)]
    pub checksum_type: String,
    pub version: String,
    #[allow(dead_code)]
    pub arch: String,
}

/// Parse primary.xml.gz content (metadata XML) to find a package by name.
/// Returns location href, checksum, checksum_type, version, arch.
pub(crate) fn parse_primary_for_package(
    body: &str,
    pkg_name: &str,
) -> SpmResult<PrimaryInfo> {
    let mut in_package = false;
    let mut matched = false;
    let mut location = String::new();
    let mut checksum = String::new();
    let mut checksum_type = String::new();
    let mut version = String::new();
    let mut arch = String::new();
    let mut in_checksum = false;

    for line in body.lines() {
        let trimmed = line.trim();

        if !in_package && trimmed.starts_with("<package") {
            in_package = true;
            matched = false;
            location.clear();
            checksum.clear();
            checksum_type.clear();
            version.clear();
            arch.clear();
            continue;
        }
        if in_package && trimmed.starts_with("</package>") {
            if matched && !location.is_empty() {
                return Ok(PrimaryInfo { location, checksum, checksum_type, version, arch });
            }
            in_package = false;
            continue;
        }
        if !in_package {
            continue;
        }

        if trimmed.starts_with("<name>") {
            let name = trimmed
                .strip_prefix("<name>")
                .and_then(|s| s.strip_suffix("</name>"))
                .unwrap_or("");
            if name == pkg_name {
                matched = true;
            }
            continue;
        }
        if trimmed.starts_with("<location") {
            let href = extract_attr(trimmed, "href");
            if !href.is_empty() {
                location = href;
            }
            continue;
        }
        if trimmed.starts_with("<version") {
            version = extract_attr(trimmed, "ver");
            continue;
        }
        if trimmed.starts_with("<arch>") {
            arch = trimmed
                .strip_prefix("<arch>")
                .and_then(|s| s.strip_suffix("</arch>"))
                .unwrap_or("")
                .to_string();
            continue;
        }
        if trimmed.starts_with("<checksum") {
            in_checksum = true;
            checksum_type = extract_attr(trimmed, "type");
            if let Some(text_start) = trimmed.find('>') {
                let after_tag = &trimmed[text_start + 1..];
                if let Some(text_end) = after_tag.find('<') {
                    checksum = after_tag[..text_end].to_string();
                }
            }
            continue;
        }
        if trimmed.starts_with("</checksum>") {
            in_checksum = false;
            continue;
        }
        if in_checksum && !trimmed.starts_with('<') {
            checksum.push_str(trimmed);
        }
    }

    Err(SpmError::package_not_found(format!(
        "Package '{pkg_name}' not found in RPM-MD repository metadata"
    )))
}

/// Extract an attribute value from an XML tag like `<foo bar="val">`
fn extract_attr(tag: &str, attr: &str) -> String {
    let pattern = format!("{attr}=\"");
    if let Some(start) = tag.find(&pattern) {
        let value_start = start + pattern.len();
        if let Some(end) = tag[value_start..].find('"') {
            return tag[value_start..value_start + end].to_string();
        }
    }
    String::new()
}

/// Download and decompress a gzipped file from a repo URL.
/// Returns the decompressed content as a String.
#[allow(dead_code)]
pub(crate) fn fetch_gz_to_string(base_url: &str, path: &str) -> SpmResult<String> {
    let url = if base_url.ends_with('/') {
        format!("{}{}", base_url, path)
    } else {
        format!("{}/{}", base_url, path)
    };

    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| SpmError::network(format!("Failed to create HTTP client: {e}")))?;
    let response = client
        .get(&url)
        .send()
        .map_err(|e| SpmError::network(format!("Failed to fetch {url}: {e}")))?;
    let max_size: u64 = 64 * 1024 * 1024;
    let body = response
        .bytes()
        .map_err(|e| SpmError::network(format!("Failed to read {url}: {e}")))?;
    if body.len() as u64 > max_size {
        return Err(SpmError::network(format!("Response too large for {url}")));
    }

    let mut decoder = flate2::read::GzDecoder::new(&body[..]);
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .map_err(|e| SpmError::compression(format!("Failed to decompress {url}: {e}")))?;

    Ok(text)
}

/// Download a file (not gzipped) from a repo URL to a Vec<u8>.
#[allow(dead_code)]
pub(crate) fn fetch_file_to_vec(base_url: &str, path: &str) -> SpmResult<Vec<u8>> {
    let url = if base_url.ends_with('/') {
        format!("{}{}", base_url, path)
    } else {
        format!("{}/{}", base_url, path)
    };

    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| SpmError::network(format!("Failed to create HTTP client: {e}")))?;
    let response = client
        .get(&url)
        .send()
        .map_err(|e| SpmError::network(format!("Failed to fetch {url}: {e}")))?;
    let max_size: u64 = 512 * 1024 * 1024;
    let body = response
        .bytes()
        .map_err(|e| SpmError::network(format!("Failed to read {url}: {e}")))?;
    if body.len() as u64 > max_size {
        return Err(SpmError::network(format!("Response too large for {url}")));
    }
    Ok(body.to_vec())
}

/// Verify a SHA256 checksum.
#[allow(dead_code)]
pub(crate) fn verify_sha256(data: &[u8], expected: &str) -> SpmResult<()> {
    let actual = crate::util::hash::sha256_hex(data);
    if actual != expected {
        return Err(SpmError::other(format!(
            "SHA256 mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

/// Download a package from an RPM-MD repository by name.
/// Returns the .rpm file bytes.
pub(crate) fn download_rpm_from_repo(
    repo_base_url: &str,
    pkg_name: &str,
) -> SpmResult<Vec<u8>> {
    // Step 1: Fetch repomd.xml
    let repomd_url = format!("{}/repodata/repomd.xml", repo_base_url.trim_end_matches('/'));
    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|e| SpmError::network(format!("Failed to create HTTP client: {e}")))?;
    let repomd_resp = client
        .get(&repomd_url)
        .send()
        .map_err(|e| SpmError::network(format!("Failed to fetch repomd.xml: {e}")))?;
    let max_size: u64 = 16 * 1024 * 1024;
    let repomd_body = repomd_resp
        .bytes()
        .map_err(|e| SpmError::network(format!("Failed to read repomd.xml: {e}")))?;
    if repomd_body.len() as u64 > max_size {
        return Err(SpmError::network(format!("repomd.xml too large")));
    }

    let repomd_text = String::from_utf8_lossy(&repomd_body);

    // Step 2: Parse repomd.xml to find primary.xml.gz
    let (primary_path, primary_cs, _primary_cs_type) = parse_repomd(&repomd_text)?;

    // Step 3: Fetch and decompress primary.xml.gz
    let primary_body = fetch_gz_to_string(repo_base_url, &primary_path)?;

    // Step 3b: Verify primary.xml.gz checksum
    if !primary_cs.is_empty() {
        if let Err(e) = verify_sha256(primary_body.as_bytes(), &primary_cs) {
            tracing::warn!("primary.xml.gz checksum mismatch for repo at {repo_base_url}: {e}");
        }
    }

    // Step 4: Find package in primary.xml and get location
    let info = parse_primary_for_package(&primary_body, pkg_name)?;

    // Step 5: Download the actual RPM
    let rpm_url = if repo_base_url.ends_with('/') {
        format!("{}{}", repo_base_url, info.location)
    } else {
        format!("{}/{}", repo_base_url, info.location)
    };

    let rpm_body = fetch_file_to_vec(repo_base_url, &info.location)?;

    // Step 6: Verify RPM checksum
    if !info.checksum.is_empty() {
        verify_sha256(&rpm_body, &info.checksum)?;
    }

    tracing::info!(
        "Downloaded {pkg_name} {version} from {rpm_url}",
        version = info.version
    );
    Ok(rpm_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repomd_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd>
  <data type="primary">
    <location href="repodata/abc-primary.xml.gz"/>
    <checksum type="sha256">deadbeef</checksum>
  </data>
</repomd>"#;
        let (loc, cs, cs_type) = parse_repomd(xml).unwrap();
        assert_eq!(loc, "repodata/abc-primary.xml.gz");
        assert_eq!(cs, "deadbeef");
        assert_eq!(cs_type, "sha256");
    }

    #[test]
    fn test_parse_repomd_no_primary() {
        let xml = r#"<repomd>
  <data type="other">
    <location href="repodata/other.xml.gz"/>
  </data>
</repomd>"#;
        assert!(parse_repomd(xml).is_err());
    }

    #[test]
    fn test_parse_primary_find_package() {
        let xml = r#"<metadata>
  <package type="rpm">
    <name>figlet</name>
    <version epoch="0" ver="2.2.5" rel="1.fc40"/>
    <arch>x86_64</arch>
    <location href="Packages/f/figlet-2.2.5-1.fc40.x86_64.rpm"/>
    <checksum type="sha256">abc123</checksum>
  </package>
  <package type="rpm">
    <name>hello</name>
    <version epoch="0" ver="2.12.1" rel="1.fc40"/>
    <arch>x86_64</arch>
    <location href="Packages/h/hello-2.12.1-1.fc40.x86_64.rpm"/>
    <checksum type="sha256">def456</checksum>
  </package>
</metadata>"#;
        let result = parse_primary_for_package(xml, "figlet");
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.location, "Packages/f/figlet-2.2.5-1.fc40.x86_64.rpm");
        assert_eq!(info.checksum, "abc123");
        assert_eq!(info.version, "2.2.5");

        let not_found = parse_primary_for_package(xml, "nonexistent");
        assert!(not_found.is_err());
    }

    #[test]
    fn test_extract_attr() {
        assert_eq!(extract_attr(r#"<data type="primary">"#, "type"), "primary");
        assert_eq!(extract_attr(r#"<location href="foo/bar.xml"/>"#, "href"), "foo/bar.xml");
        assert_eq!(extract_attr(r#"<checksum type="sha256">"#, "type"), "sha256");
    }
}
