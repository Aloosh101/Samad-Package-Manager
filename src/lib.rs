#![allow(dead_code)]

pub mod backend;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
pub mod index;
pub mod kernel;
pub mod package;
pub mod types;
pub mod util;
pub mod store;
pub mod sandbox;
pub mod analyze;
pub mod daemon;
pub mod isolation;
pub mod cache;
pub mod output;
pub mod verify;
pub mod sync;
pub mod ux;
pub mod fsck;
pub mod bootstrap;
pub mod integration;

#[cfg(test)]
mod tests {
    #[test]
    fn test_core_types_accessible() {
        let _ = crate::types::PackageFormat::Deb;
        let _ = crate::types::TransactionAction::Install;
        let _ = crate::error::SpmError::other("test");
    }

}
