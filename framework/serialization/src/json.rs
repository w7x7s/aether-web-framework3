//! # High-Performance Zero-Copy JSON Parser and Serializer
//!
//! Provides zero-copy deserialization for borrowed strings (&'de str) and streaming serialization.

use aether_core::error::AetherError;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Deserializes a borrowed JSON slice into target DTO without copying string fields.
#[inline(always)]
pub fn from_slice<'a, T: Deserialize<'a>>(slice: &'a [u8]) -> Result<T, AetherError> {
    serde_json::from_slice(slice).map_err(|e| AetherError::ValidationFailed(e.to_string()))
}

/// Serializes a DTO directly into a Bytes buffer.
#[inline(always)]
pub fn to_bytes<T: Serialize>(value: &T) -> Result<Bytes, AetherError> {
    serde_json::to_vec(value)
        .map(Bytes::from)
        .map_err(|e| AetherError::ValidationFailed(e.to_string()))
}
