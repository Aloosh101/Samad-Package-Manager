pub mod args;
pub mod client;
pub mod group;

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn test_submodule_access() {
        let result = crate::cli::args::SpmArgs::try_parse_from(["spm", "install", "pkg"]);
        assert!(result.is_ok());
    }
}
