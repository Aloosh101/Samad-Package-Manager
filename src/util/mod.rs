pub mod backend;
pub mod fs;
pub mod hash;
pub mod process;
pub mod user;

#[cfg(test)]
mod tests {
    #[test]
    fn test_submodules_accessible() {
        let hash = crate::util::hash::hash_bytes(b"test");
        assert_eq!(hash.len(), 64);
    }
}
