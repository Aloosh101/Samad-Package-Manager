pub mod deb;
pub mod rpm;

pub use deb::extract_deb;
pub use rpm::extract_rpm;
