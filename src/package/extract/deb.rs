use std::io::{Cursor, Read};
use std::path::Path;

use crate::error::{SpmError, SpmResult};

const AR_MAGIC: &[u8; 8] = b"!<arch>\n";

pub async fn extract_deb(deb_data: &[u8], dest: &Path) -> SpmResult<()> {
    let data = deb_data.to_vec();
    tokio::task::spawn_blocking({
        let dest = dest.to_path_buf();
        move || extract_deb_sync(&data, &dest)
    })
    .await?
}

pub fn extract_deb_sync(deb_data: &[u8], dest: &Path) -> SpmResult<()> {
    if deb_data.len() < 8 || &deb_data[..8] != AR_MAGIC {
        return Err(SpmError::invalid_format("Invalid .deb file: bad ar magic"));
    }

    let mut cursor = Cursor::new(deb_data);
    cursor.read_exact(&mut [0u8; 8]).unwrap();

    loop {
        let Some(entry) = read_ar_entry(&mut cursor)? else {
            break;
        };

        let name = normalize_ar_name(&entry.name);
        if name == "control.tar.xz" {
            extract_tar_xz(&entry.data, &dest.join("CONTROL"))?;
        } else if name == "data.tar.xz" {
            extract_tar_xz(&entry.data, dest)?;
        } else if name == "control.tar.gz" || name == "control.tar" {
            extract_tar_any(&entry.data, &dest.join("CONTROL"))?;
        } else if name == "data.tar.gz" || name == "data.tar" {
            extract_tar_any(&entry.data, dest)?;
        } else if name == "control.tar.zst" {
            extract_tar_zstd(&entry.data, &dest.join("CONTROL"))?;
        } else if name == "data.tar.zst" {
            extract_tar_zstd(&entry.data, dest)?;
        }
    }

    Ok(())
}

struct ArEntry {
    name: String,
    data: Vec<u8>,
}

fn read_ar_entry(reader: &mut dyn Read) -> SpmResult<Option<ArEntry>> {
    let mut header = [0u8; 60];
    match reader.read_exact(&mut header) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SpmError::other(format!("Failed to read ar header: {e}"))),
    }

    let name_bytes = &header[0..16];
    let name = String::from_utf8_lossy(name_bytes).trim().to_string();
    let size_str = std::str::from_utf8(&header[48..58]).unwrap_or("0").trim();
    let size: usize = size_str.parse().unwrap_or(0);

    let mut data = vec![0u8; size];
    reader
        .read_exact(&mut data)
        .map_err(|e| SpmError::other(format!("Failed to read ar data: {e}")))?;

    if size % 2 == 1 {
        let mut pad = [0u8; 1];
        reader
            .read_exact(&mut pad)
            .map_err(|e| SpmError::other(format!("Failed to read ar padding: {e}")))?;
    }

    Ok(Some(ArEntry { name, data }))
}

fn normalize_ar_name(name: &str) -> &str {
    if let Some(end) = name.find('/') {
        name[..end].trim()
    } else {
        name.trim()
    }
}

fn extract_tar_xz(data: &[u8], dest: &Path) -> SpmResult<()> {
    let mut decoder = xz2::read::XzDecoder::new(data);
    let mut archive = tar::Archive::new(&mut decoder);
    archive
        .unpack(dest)
        .map_err(|e| SpmError::compression(format!("Failed to unpack tar.xz: {e}")))?;
    Ok(())
}

fn extract_tar_any(data: &[u8], dest: &Path) -> SpmResult<()> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut archive = tar::Archive::new(&mut decoder);
        archive
            .unpack(dest)
            .map_err(|e| SpmError::compression(format!("Failed to unpack tar.gz: {e}")))?;
    } else {
        let mut archive = tar::Archive::new(data);
        archive
            .unpack(dest)
            .map_err(|e| SpmError::compression(format!("Failed to unpack tar: {e}")))?;
    }
    Ok(())
}

fn extract_tar_zstd(data: &[u8], dest: &Path) -> SpmResult<()> {
    let mut decoder = zstd::Decoder::new(data)
        .map_err(|e| SpmError::compression(format!("Failed to create zstd decoder: {e}")))?;
    let mut archive = tar::Archive::new(&mut decoder);
    archive
        .unpack(dest)
        .map_err(|e| SpmError::compression(format!("Failed to unpack tar.zst: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn build_test_ar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = Vec::new();
        archive.extend_from_slice(b"!<arch>\n");
        for (name, data) in files {
            let header = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", name, "0", "0", "0", "100644", data.len());
            assert_eq!(header.as_bytes().len(), 60, "AR header must be exactly 60 bytes");
            archive.extend_from_slice(header.as_bytes());
            archive.extend_from_slice(data);
            if data.len() % 2 == 1 {
                archive.push(b'\n');
            }
        }
        archive
    }

    fn build_tar_xz(data: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut builder = tar::Builder::new(Vec::new());
        for (name, content) in data {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *content).unwrap();
        }
        builder.finish().unwrap();
        let raw_tar = builder.into_inner().unwrap();

        let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
        encoder.write_all(&raw_tar).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_extract_deb_invalid_magic() {
        let result = extract_deb_sync(b"invalid", Path::new("/tmp"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad ar magic"));
    }

    #[test]
    fn test_extract_deb_empty_archive() {
        let dir = tempfile::tempdir().unwrap();
        let ar = build_test_ar(&[]);
        let result = extract_deb_sync(&ar, dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_deb_with_control() {
        let dir = tempfile::tempdir().unwrap();
        let control_data = b"Package: test-pkg\nVersion: 1.0\n";
        let control_tar_xz = build_tar_xz(&[("control", &control_data[..])]);
        let ar = build_test_ar(&[("control.tar.xz", &control_tar_xz)]);

        extract_deb_sync(&ar, dir.path()).unwrap();

        let control_path = dir.path().join("CONTROL").join("control");
        assert!(control_path.exists());
        let content = fs::read_to_string(control_path).unwrap();
        assert!(content.contains("Package: test-pkg"));
    }

    #[test]
    fn test_extract_deb_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let data_tar_xz = build_tar_xz(&[("usr/bin/hello", b"#!/bin/sh\necho hello")]);
        let ar = build_test_ar(&[("data.tar.xz", &data_tar_xz)]);

        extract_deb_sync(&ar, dir.path()).unwrap();

        let extracted = dir.path().join("usr/bin/hello");
        assert!(extracted.exists());
        let content = fs::read_to_string(extracted).unwrap();
        assert_eq!(content, "#!/bin/sh\necho hello");
    }
}
