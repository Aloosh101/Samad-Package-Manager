use std::path::Path;

pub fn is_elf(path: &Path) -> bool {
    let mut buf = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut buf)
        })
        .is_ok() && buf == [0x7f, b'E', b'L', b'F']
}

pub fn whoami() -> String {
    std::env::var("USER").unwrap_or("root".to_string())
}

pub fn host_arch() -> String {
    std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or("x86_64".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_is_elf_actual_elf() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Write minimal ELF header
        let elf_header = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        std::fs::write(tmp.path(), elf_header).unwrap();
        assert!(is_elf(tmp.path()));
    }

    #[test]
    fn test_is_elf_non_elf() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"not an elf file").unwrap();
        assert!(!is_elf(tmp.path()));
    }

    #[test]
    fn test_is_elf_nonexistent() {
        assert!(!is_elf(Path::new("/nonexistent-file-99999.elf")));
    }

    #[test]
    fn test_is_elf_empty() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"").unwrap();
        assert!(!is_elf(tmp.path()));
    }

    #[test]
    fn test_whoami_env_set() {
        std::env::set_var("USER", "testuser");
        assert_eq!(whoami(), "testuser");
    }

    #[test]
    fn test_whoami_default_root() {
        std::env::remove_var("USER");
        assert_eq!(whoami(), "root");
    }
}
