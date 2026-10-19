//! # Aether Web Framework
//!
//! High-performance Rust web framework with a Tokio multi-threaded runtime, zero-copy
//! request parsing, chunked and Content-Length body framing, and rich developer diagnostics.
//!
//! HTTP/1.1 only: HTTP/2 and HTTP/3 are not implemented. Request bodies are copied into
//! owned buffers before handlers run (the parser itself borrows from the connection buffer).
//! `aether_core::RequestArena` is available for request-scoped allocation but is not used by
//! the default server path.

pub mod app;
pub mod config;
pub mod extract;
pub mod handler;
mod static_dir;
pub mod scheduling;
pub mod test_client;

pub use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
pub use aether_core::error::AetherError;
pub use aether_http::method::Method;
pub use aether_http::multipart::{MultipartError, MultipartForm, Part};
pub use aether_http::response::Response;
pub use aether_http::static_files::{generate_etag, mime_from_extension, serve_static};
pub use aether_middleware::{CompressionConfig, CompressionMiddleware, CorsConfig, CorsMiddleware, LoggerMiddleware, SecurityHeadersMiddleware};
pub use aether_websocket as websocket;
pub use aether_websocket::{Message as WsMessage, WebSocket, WebSocketLimits, WsError};
pub use app::{AetherApp, AetherServer, AppStateContainer, IntoResponse, MiddlewareEntry, Next, RequestContext, RouteGroup, WsHandlerFn};
pub use config::AetherConfig;
pub use extract::{Form, FromRequest, FromRequestParts, Json, Path, Query, State};
pub use handler::Handler;
pub use scheduling::{AdmissionGuard, AdmitFailure, SliceProgress, SchedulingLayer};
pub use test_client::{TestClient, TestRequest, TestResponse};
pub use aether_scheduler::{
    PolicyKind, ResourceConfig as SchedulingResourceConfig, SchedulerMetrics as SchedulingMetrics,
    SchedulingConfig, Work as SchedWork,
};

pub mod prelude {
    pub use crate::app::{AetherApp, AetherServer, AppStateContainer, IntoResponse, MiddlewareEntry, Next, RequestContext, RouteGroup, WsHandlerFn};
    pub use crate::config::AetherConfig;
    pub use crate::extract::{Form, FromRequest, FromRequestParts, Json, Path, Query, State};
    pub use crate::handler::Handler;
    pub use crate::test_client::{TestClient, TestRequest, TestResponse};
    pub use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
    pub use aether_core::error::AetherError;
    pub use aether_http::method::Method;
    pub use aether_http::multipart::{MultipartError, MultipartForm, Part};
    pub use aether_http::response::Response;
    pub use aether_http::static_files::{generate_etag, mime_from_extension, serve_static};
    pub use aether_middleware::{CompressionConfig, CompressionMiddleware, CorsConfig, CorsMiddleware, LoggerMiddleware, SecurityHeadersMiddleware};
    pub use aether_websocket::{Message as WsMessage, WebSocket, WebSocketLimits, WsError};
}
