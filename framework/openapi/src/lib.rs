//! # Aether OpenAPI v3.1 Spec Generator
//!
//! Generates full OpenAPI JSON Schemas at build time or startup,
//! completely isolated from the runtime request processing path.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, Debug, Clone)]
pub struct OpenApiSpec {
    pub openapi: &'static str,
    pub info: Info,
    /// `BTreeMap`, not `HashMap`: a spec is a build artifact that gets diffed and
    /// version-controlled, so the key order in the emitted JSON must be stable across
    /// runs and processes.
    pub paths: BTreeMap<String, PathItem>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Info {
    pub title: String,
    pub version: String,
    pub description: String,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct PathItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation>,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct Operation {
    pub summary: String,
    pub operation_id: String,
    pub responses: BTreeMap<String, ResponseSpec>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ResponseSpec {
    pub description: String,
}

impl OpenApiSpec {
    pub fn new(title: &str, version: &str) -> Self {
        Self {
            openapi: "3.1.0",
            info: Info {
                title: title.to_string(),
                version: version.to_string(),
                description: "Native Aether Ultra-High-Performance REST API".to_string(),
            },
            paths: BTreeMap::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}
