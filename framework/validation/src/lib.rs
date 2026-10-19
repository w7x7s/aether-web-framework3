//! # Aether Validation Crate

pub mod simd;
pub mod validator;

pub use simd::{validate_ascii_fast, validate_email_format};
pub use validator::{BoundedStr, Email, Validate};
