use std::path::PathBuf;

const BUNDLED_DIR: &str = "/usr/libexec/spm/backend";

pub fn resolve(name: &str) -> PathBuf {
    if name.contains('/') || name.contains("..") || name.contains('\0') || name.is_empty() {
        return PathBuf::from("/dev/null/spm-backend-invalid");
    }

    // 1. Store-managed backend (self-contained copy, survives system RPM removal)
    let store = crate::config::paths::store_backend_dir()
        .join(name)
        .join("bin")
        .join(name);
    if store.exists() {
        return store;
    }

    // 2. Bundled backend (shipped with SPM, read-only)
    let bundled = PathBuf::from(BUNDLED_DIR).join(name);
    if bundled.exists() {
        return bundled;
    }

    // 3. No fallback to system PATH — SPM is the sole package manager.
    // Return a safe non-existent path so Command will fail cleanly.
    PathBuf::from("/dev/null/spm-backend-missing")
}

pub fn bundled_dir() -> &'static str {
    BUNDLED_DIR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_returns_safe_path_for_missing() {
        let p = resolve("nonexistent-backend");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-missing"));
    }

    #[test]
    fn test_resolve_rejects_slash() {
        let p = resolve("foo/bar");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-invalid"));
    }

    #[test]
    fn test_resolve_rejects_dotdot() {
        let p = resolve("..");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-invalid"));
    }

    #[test]
    fn test_resolve_rejects_null() {
        let p = resolve("bad\0name");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-invalid"));
    }

    #[test]
    fn test_resolve_empty() {
        let p = resolve("");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-invalid"));
    }

    #[test]
    fn test_resolve_missing_backend() {
        let p = resolve("nonexistent-backend");
        assert_eq!(p, PathBuf::from("/dev/null/spm-backend-missing"));
    }
}
