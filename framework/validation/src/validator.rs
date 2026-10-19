//! # Compile-Time Monomorphic Validation Engine

use crate::simd::{validate_ascii_fast, validate_email_format};
use aether_core::error::AetherError;

pub trait Validate {
    fn validate(&self) -> Result<(), AetherError>;
}

/// A validated String type that enforces UTF-8 ASCII & length bounds without re-allocating.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedStr<const MIN: usize, const MAX: usize>(String);

impl<const MIN: usize, const MAX: usize> BoundedStr<MIN, MAX> {
    pub fn try_new(s: String) -> Result<Self, AetherError> {
        let len = s.len();
        if len < MIN || len > MAX {
            return Err(AetherError::ValidationFailed(format!(
                "String length {} out of bounds [{}, {}]",
                len, MIN, MAX
            )));
        }
        if !validate_ascii_fast(s.as_bytes()) {
            return Err(AetherError::ValidationFailed("Non-ASCII string".into()));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated Email type
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn try_new(s: String) -> Result<Self, AetherError> {
        if !validate_email_format(&s) {
            // Do not echo the raw input into the error: it is caller-controlled and
            // unbounded, so interpolating it leaks request data into logs and response
            // bodies and invites log injection. The field name is enough to locate the fault.
            return Err(AetherError::ValidationFailed(
                "Invalid email format".into(),
            ));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
