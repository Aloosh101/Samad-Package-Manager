use std::io::Read;
use sha2::Digest;

pub fn hash_file(path: &str) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

pub fn hash_dir(dir: &std::path::Path) -> Result<String, std::io::Error> {
    use std::io::Read;
    let mut hasher = blake3::Hasher::new();
    let entries: Vec<_> = walkdir::WalkDir::new(dir)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() || e.file_type().is_symlink())
        .collect();
    for entry in &entries {
        let relative = entry.path().strip_prefix(dir)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if relative.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(std::io::Error::other(format!(
                "Path traversal detected: '{}'", relative.display()
            )));
        }
        hasher.update(relative.to_string_lossy().as_bytes());
        if entry.file_type().is_file() {
            let mut f = std::fs::File::open(entry.path())?;
            let mut buf = [0; 8192];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 { break; }
                hasher.update(&buf[..n]);
            }
        } else if entry.file_type().is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            hasher.update(target.to_string_lossy().as_bytes());
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(hasher.finalize().as_slice())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_bytes_known() {
        let hash = hash_bytes(b"hello world");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_bytes_empty() {
        let hash = hash_bytes(b"");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_hash_bytes_deterministic() {
        let a = hash_bytes(b"test data");
        let b = hash_bytes(b"test data");
        assert_eq!(a, b);
    }

    #[test]
    fn test_hash_bytes_different() {
        let a = hash_bytes(b"data1");
        let b = hash_bytes(b"data2");
        assert_ne!(a, b);
    }

    #[test]
    fn test_hash_file() {
        let dir = std::env::temp_dir().join("spm-hash-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        std::fs::write(&path, b"hello file").unwrap();

        let hash = hash_file(path.to_str().unwrap()).unwrap();
        assert_eq!(hash.len(), 64);

        let expected = hash_bytes(b"hello file");
        assert_eq!(hash, expected);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hash_file_nonexistent() {
        let result = hash_file("/tmp/nonexistent-file-for-test-12345");
        assert!(result.is_err());
    }
}
