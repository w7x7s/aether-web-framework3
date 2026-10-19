//! # Aether Serialization Crate

pub mod json;

pub use json::{from_slice, to_bytes};
