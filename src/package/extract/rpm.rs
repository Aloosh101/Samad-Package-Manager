use std::io::Read;
use std::path::Path;

use crate::error::{SpmError, SpmResult};

const RPM_MAGIC: &[u8; 4] = b"\xed\xab\xee\xdb";

pub async fn extract_rpm(rpm_data: &[u8], dest: &Path) -> SpmResult<()> {
    if rpm_data.len() < 4 || &rpm_data[..4] != RPM_MAGIC {
        return Err(SpmError::invalid_format("Invalid .rpm file: bad magic"));
    }

    let data = rpm_data.to_vec();
    let dest1 = dest.to_path_buf();
    tokio::task::spawn_blocking(move || extract_cpio_payload(&data, &dest1))
        .await?
}

fn extract_cpio_payload(rpm_data: &[u8], dest: &Path) -> SpmResult<()> {
    let payload = extract_payload(rpm_data)?;
    decompress_cpio(&payload, dest)
}

fn extract_payload(data: &[u8]) -> SpmResult<Vec<u8>> {
    let mut offset = 96usize;

    // Skip signature header
    let (_sig_offset, sig_size) = read_index_header(data, offset)?;
    offset = offset + 16 + sig_size;

    // Read header
    let (_hdr_offset, hdr_size) = read_index_header(data, offset)?;
    offset = offset + 16 + hdr_size;

    // Remaining data is the payload
    let payload = data[offset..].to_vec();
    Ok(payload)
}

fn read_index_header(data: &[u8], offset: usize) -> SpmResult<(usize, usize)> {
    if offset + 16 > data.len() {
        return Err(SpmError::invalid_format("RPM data truncated at index header"));
    }

    let nindex = u32::from_be_bytes([
        data[offset + 8],
        data[offset + 9],
        data[offset + 10],
        data[offset + 11],
    ]) as usize;
    let hsize = u32::from_be_bytes([
        data[offset + 12],
        data[offset + 13],
        data[offset + 14],
        data[offset + 15],
    ]) as usize;

    if nindex > 65536 || hsize > 4 * 1024 * 1024 {
        return Err(SpmError::invalid_format(format!(
            "RPM header too large: {nindex} entries, {hsize} byte store"
        )));
    }

    let store_size = (nindex * 16 + hsize + 7) & !7;
    Ok((offset + 16, store_size))
}

fn decompress_cpio(data: &[u8], target: &Path) -> SpmResult<()> {
    let decompressed = if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| SpmError::compression(format!("gzip decompression failed: {e}")))?;
        out
    } else if data.len() > 4 && data.starts_with(b"\xfd7zXZ") {
        let mut decoder = xz2::read::XzDecoder::new(data);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| SpmError::compression(format!("xz decompression failed: {e}")))?;
        out
    } else if data.len() > 4 {
        match zstd::decode_all(data) {
            Ok(out) => out,
            Err(_) => data.to_vec(),
        }
    } else {
        data.to_vec()
    };

    if decompressed.len() < 6 {
        return Ok(());
    }

    let mut offset = 0;
    while offset + 110 < decompressed.len() {
        let magic = &decompressed[offset..offset + 6];
        if magic != b"070701" && magic != b"070702" {
            break;
        }

        let read_u32 = |pos: usize| -> u32 {
            u32::from_be_bytes([
                decompressed[pos],
                decompressed[pos + 1],
                decompressed[pos + 2],
                decompressed[pos + 3],
            ])
        };

        let mode = read_u32(offset + 14);
        let namesize = read_u32(offset + 76);
        let filesize = read_u32(offset + 80);

        if namesize == 0 {
            break;
        }

        let name_offset = offset + 110;
        let name_end = name_offset + namesize as usize - 1;
        if name_end > decompressed.len() {
            break;
        }
        let name =
            String::from_utf8_lossy(&decompressed[name_offset..name_end]).to_string();

        let pad = (4 - (110 + namesize as usize) % 4) % 4;
        let data_offset = name_offset + namesize as usize + pad;
        let data_end = data_offset + filesize as usize;

        if data_end > decompressed.len() {
            break;
        }

        if !name.is_empty() && name != "." && name != "/" && name != "TRAILER!!!" {
            let clean_name = name.strip_prefix('/').unwrap_or(&name);
            let target_path = target.join(clean_name);
            crate::package::store::sanitize_relative(&target_path)?;

            if mode & 0o040_000 != 0 {
                std::fs::create_dir_all(&target_path).map_err(|e| {
                    SpmError::other(format!("Failed to create dir {:?}: {e}", target_path))
                })?;
            } else if filesize > 0 {
                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        SpmError::other(format!(
                            "Failed to create parent for {:?}: {e}",
                            target_path
                        ))
                    })?;
                }
                std::fs::write(&target_path, &decompressed[data_offset..data_end])
                    .map_err(|e| {
                        SpmError::other(format!("Failed to write {:?}: {e}", target_path))
                    })?;
            }
        }

        let entry_size = 110 + namesize as usize + pad + filesize as usize;
        let entry_pad = (4 - entry_size % 4) % 4;
        offset += entry_size + entry_pad;

        if name == "TRAILER!!!" {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_cpio_entry(magic: &[u8; 6], name: &str, data: &[u8], mode: u32) -> Vec<u8> {
        let namesize = name.len() + 1;
        let filesize = data.len();
        let pad_name = (4 - (110 + namesize) % 4) % 4;
        let pad_data = (4 - filesize % 4) % 4;
        let entry_size = 110 + namesize + pad_name + filesize + pad_data;

        let mut entry = vec![0u8; entry_size];
        entry[0..6].copy_from_slice(magic);
        entry[14..18].copy_from_slice(&mode.to_be_bytes());
        entry[30..34].copy_from_slice(&1u32.to_be_bytes());
        entry[76..80].copy_from_slice(&(namesize as u32).to_be_bytes());
        entry[80..84].copy_from_slice(&(filesize as u32).to_be_bytes());
        entry[110..110 + namesize - 1].copy_from_slice(name.as_bytes());
        entry[110 + namesize - 1] = 0;

        let data_start = 110 + namesize + pad_name;
        entry[data_start..data_start + filesize].copy_from_slice(data);

        entry
    }

    fn build_cpio_trailer() -> Vec<u8> {
        let magic: &[u8; 6] = b"070701";
        build_cpio_entry(magic, "TRAILER!!!", b"", 0)
    }

    fn build_cpio_archive(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut archive = Vec::new();
        let magic: &[u8; 6] = b"070701";
        for (name, data, mode) in files {
            archive.extend(build_cpio_entry(magic, name, data, *mode));
        }
        archive.extend(build_cpio_trailer());
        archive
    }

    #[test]
    fn test_decompress_cpio_empty() {
        let result = decompress_cpio(b"", Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_short() {
        let result = decompress_cpio(b"short", Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_invalid_magic() {
        let mut data = b"XXXXXX".to_vec();
        data.extend(std::iter::repeat_n(0u8, 110));
        let result = decompress_cpio(&data, Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_file() {
        let dir = tempfile::tempdir().unwrap();
        let archive = build_cpio_archive(&[("file.txt", b"hello world", 0o_100_644)]);
        decompress_cpio(&archive, dir.path()).unwrap();
        let extracted = dir.path().join("file.txt");
        assert!(extracted.exists());
        assert_eq!(std::fs::read_to_string(extracted).unwrap(), "hello world");
    }

    #[test]
    fn test_decompress_cpio_directory() {
        let dir = tempfile::tempdir().unwrap();
        let archive = build_cpio_archive(&[("subdir", b"", 0o_040_755)]);
        decompress_cpio(&archive, dir.path()).unwrap();
        assert!(dir.path().join("subdir").exists());
        assert!(dir.path().join("subdir").is_dir());
    }

    #[test]
    fn test_decompress_cpio_nested() {
        let dir = tempfile::tempdir().unwrap();
        let archive = build_cpio_archive(&[
            ("usr", b"", 0o_040_755),
            ("usr/bin", b"", 0o_040_755),
            ("usr/bin/hello", b"#!/bin/sh\necho hello", 0o_100_755),
        ]);
        decompress_cpio(&archive, dir.path()).unwrap();
        assert!(dir.path().join("usr/bin/hello").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("usr/bin/hello")).unwrap(),
            "#!/bin/sh\necho hello"
        );
    }

    #[test]
    fn test_decompress_cpio_strips_leading_slash() {
        let dir = tempfile::tempdir().unwrap();
        let archive = build_cpio_archive(&[("/etc/nginx.conf", b"server { }", 0o_100_644)]);
        decompress_cpio(&archive, dir.path()).unwrap();
        assert!(dir.path().join("etc/nginx.conf").exists());
    }

    #[test]
    fn test_decompress_cpio_070702_magic() {
        let dir = tempfile::tempdir().unwrap();
        let magic: &[u8; 6] = b"070702";
        let entry = build_cpio_entry(magic, "test.bin", b"\x00\x01\x02\x03", 0o_100_644);
        let mut archive = entry;
        archive.extend(build_cpio_trailer());
        decompress_cpio(&archive, dir.path()).unwrap();
        assert!(dir.path().join("test.bin").exists());
        assert_eq!(std::fs::read(dir.path().join("test.bin")).unwrap(), b"\x00\x01\x02\x03");
    }

    #[test]
    fn test_extract_payload_invalid() {
        let result = extract_payload(b"short");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_extract_rpm_invalid_magic() {
        let result = extract_rpm(b"invalid", Path::new("/tmp")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bad magic"));
    }
}
