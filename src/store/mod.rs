//! Content-Addressed Store module.
//!
//! Provides a unified content-addressed storage layer for shared libraries
//! across all supported package formats (Deb, Rpm, Sam).  The module is
//! organized into three sub-modules:
//!
//! * [`deduplication`] – [`StoreManager`], [`OriginStore`], [`SharedLibStore`],
//!   [`SharedLibEntry`], and [`Provider`].
//! * [`hardlink`] – [`deduplicate_library`] that BLAKE3-hashes `.so` files
//!   and creates hardlinks when content already exists.
//! * [`gc`] – Garbage collection that removes unreferenced shared libraries.
//!
//! # Re-exports
//!
//! The most important types are re-exported at the `store` level for
//! convenience.

pub mod deduplication;
pub mod gc;
pub mod hardlink;

pub use deduplication::{OriginStore, Provider, SharedLibEntry, SharedLibStore, StoreManager};
pub use gc::GcStats;
pub use hardlink::deduplicate_library;
