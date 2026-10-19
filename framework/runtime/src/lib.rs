//! # Aether Runtime Crate
//!
//! Legacy prototype connection loop, kept only as a workspace member. Nothing in this
//! repository uses it: the production serve path lives in `aether::app` (`AetherApp::serve`),
//! which adds chunked-body decoding, Host enforcement, keep-alive/timeout handling,
//! backpressure-bounded writes, scheduler admission, and middleware. `handle_connection`
//! here parses each TCP read slice in isolation (split requests are silently dropped),
//! never enforces `Host`, and applies no I/O timeouts. Do not build on it; prefer
//! [`aether::AetherApp`]. Do not treat it as supported API.

pub mod worker;

pub use worker::handle_connection;
