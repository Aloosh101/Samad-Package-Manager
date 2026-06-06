pub mod build;
pub mod cleanup;
pub mod deb;
pub mod fetch;
pub mod hooks;
pub mod install;
pub mod query;
pub mod resolver;
pub mod solver;
pub mod rpm;
pub mod sam;
pub mod scripts;
pub mod store;
pub mod transaction;
pub mod triggers;

#[cfg(test)]
mod tests {
    #[test]
    fn test_public_api_accessible() {
        // remove_package is re-exported via `pub use remove::remove_package`
        // so it's accessible at install::remove_package, not install::remove::remove_package
        let _: fn(&str) -> crate::error::SpmResult<()> = crate::package::install::remove_package;
        let _ = crate::package::resolver::bump_dep_cache_epoch;
    }
}
