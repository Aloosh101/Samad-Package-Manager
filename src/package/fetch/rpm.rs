use std::io::Read;

use crate::error::{SpmError, SpmResult};
use crate::package::fetch::gs;

pub fn fetch_rpm_package(
    name: &str,
    version: Option<&str>,
    repo_url: &str,
) -> SpmResult<Vec<u8>> {
    let base = repo_url.trim_end_matches('/');

    let repomd_url = format!("{base}/repodata/repomd.xml");
    let repomd_body = download_bytes(&repomd_url)?;
    let repomd_text = String::from_utf8_lossy(&repomd_body);

    let (primary_path, primary_cs, _cs_type) = gs::parse_repomd(&repomd_text)?;

    let primary_url = if primary_path.starts_with('/') {
        format!("{base}{primary_path}")
    } else if primary_path.starts_with("repodata/") {
        format!("{base}/{primary_path}")
    } else {
        format!("{base}/repodata/{primary_path}")
    };
    let primary_compressed = download_bytes(&primary_url)?;
    let mut decoder = flate2::read::GzDecoder::new(&primary_compressed[..]);
    let mut primary_text = String::new();
    decoder
        .read_to_string(&mut primary_text)
        .map_err(|e| SpmError::compression(format!("Failed to decompress primary.xml: {e}")))?;

    if !primary_cs.is_empty() {
        let actual = crate::util::hash::sha256_hex(primary_text.as_bytes());
        if actual != primary_cs {
            tracing::warn!(
                "primary.xml.gz checksum mismatch: expected {primary_cs}, got {actual}"
            );
        }
    }

    let info = gs::parse_primary_for_package(&primary_text, name)?;

    if let Some(ver) = version {
        if info.version != ver {
            return Err(SpmError::package_not_found(format!(
                "Package '{name}' version {ver} not found in repo (found version {})",
                info.version
            )));
        }
    }

    let rpm_url = if info.location.starts_with('/') {
        format!("{base}{}", info.location)
    } else {
        format!("{base}/{}", info.location)
    };
    let rpm_body = download_bytes(&rpm_url)?;

    if !info.checksum.is_empty() {
        let actual = crate::util::hash::sha256_hex(&rpm_body);
        if actual != info.checksum {
            return Err(SpmError::other(format!(
                "SHA256 mismatch for '{name}': expected {}, got {actual}",
                info.checksum
            )));
        }
    }

    Ok(rpm_body)
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
