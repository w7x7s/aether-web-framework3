//! # Aether Precompiled Dependency Injection Crate

pub mod dag;
pub mod scope;

pub use dag::{DependencyGraph, DependencyNode, NodeId};
pub use scope::DependencyScope;
