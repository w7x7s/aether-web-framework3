//! # Aether Diagnostic & Error Experience Engine
//!
//! Provides structured error codes (E1001-E5002), actionable developer hints,
//! human-readable and machine-parsable terminal diagnostics, and safe production sanitization.

use std::fmt;

/// Standardized Aether Error Code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// E1001: Server startup failed (e.g., port binding / address already in use)
    E1001ServerStartupFailed,
    /// E1002: Invalid or missing environment/configuration value
    E1002InvalidConfig,
    /// E2001: Route configuration collision or invalid pattern
    E2001RouteCollision,
    /// E2002: Route not found (HTTP 404)
    E2002RouteNotFound,
    /// E2003: HTTP Method not allowed for route (HTTP 405)
    E2003MethodNotAllowed,
    /// E3001: Malformed HTTP request syntax or RFC 9112 smuggling violation
    E3001MalformedRequest,
    /// E3002: HTTP payload budget exceeded (Headers or Body limit)
    E3002PayloadLimitExceeded,
    /// E4001: JSON or form deserialization failure
    E4001DeserializationFailed,
    /// E4002: Input validation or constraint failure
    E4002ValidationFailed,
    /// E5001: Circular dependency in DI graph
    E5001DependencyCycle,
    /// E5002: Unhandled handler error or internal server failure
    E5002InternalServerError,
}

impl ErrorCode {
    pub fn code_str(&self) -> &'static str {
        match self {
            ErrorCode::E1001ServerStartupFailed => "E1001",
            ErrorCode::E1002InvalidConfig => "E1002",
            ErrorCode::E2001RouteCollision => "E2001",
            ErrorCode::E2002RouteNotFound => "E2002",
            ErrorCode::E2003MethodNotAllowed => "E2003",
            ErrorCode::E3001MalformedRequest => "E3001",
            ErrorCode::E3002PayloadLimitExceeded => "E3002",
            ErrorCode::E4001DeserializationFailed => "E4001",
            ErrorCode::E4002ValidationFailed => "E4002",
            ErrorCode::E5001DependencyCycle => "E5001",
            ErrorCode::E5002InternalServerError => "E5002",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            ErrorCode::E1001ServerStartupFailed => "Server startup failed",
            ErrorCode::E1002InvalidConfig => "Invalid configuration parameter",
            ErrorCode::E2001RouteCollision => "Route registration conflict",
            ErrorCode::E2002RouteNotFound => "Route not found",
            ErrorCode::E2003MethodNotAllowed => "Method not allowed for route",
            ErrorCode::E3001MalformedRequest => "Malformed HTTP request syntax",
            ErrorCode::E3002PayloadLimitExceeded => "HTTP payload budget exceeded",
            ErrorCode::E4001DeserializationFailed => "JSON/Form deserialization failure",
            ErrorCode::E4002ValidationFailed => "Input validation failed",
            ErrorCode::E5001DependencyCycle => "Circular dependency detected in DI graph",
            ErrorCode::E5002InternalServerError => "Internal handler error",
        }
    }
}

/// Rich Diagnostic Report
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: ErrorCode,
    pub reason: String,
    pub context_key: Option<String>,
    pub context_val: Option<String>,
    pub hint: Option<String>,
    pub suggestions: Vec<String>,
}

impl Diagnostic {
    pub fn new(code: ErrorCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            context_key: None,
            context_val: None,
            hint: None,
            suggestions: Vec::new(),
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.context_key = Some(key.into());
        self.context_val = Some(val.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    /// Formats the diagnostic into an actionable, clean terminal string
    pub fn format_terminal(&self, color: bool) -> String {
        let code = self.code.code_str();
        let title = self.code.title();

        let prefix = if color {
            format!("\x1b[1;31mERROR [{}]\x1b[0m \x1b[1m{}\x1b[0m", code, title)
        } else {
            format!("ERROR [{}] {}", code, title)
        };

        let mut out = String::new();
        out.push_str(&prefix);
        out.push('\n');

        if let (Some(k), Some(v)) = (&self.context_key, &self.context_val) {
            out.push_str(&format!("  {}: {}\n", k, v));
        }

        out.push_str(&format!("  Reason : {}\n", self.reason));

        if let Some(ref hint) = self.hint {
            out.push_str("\n  Hint:\n");
            out.push_str(&format!("  {}\n", hint));
        }

        if !self.suggestions.is_empty() {
            out.push_str("\n  Try:\n");
            for s in &self.suggestions {
                out.push_str(&format!("  • {}\n", s));
            }
        }

        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_terminal(false))
    }
}

/// Helper for rendering visual server startup banners
pub struct ServerBanner {
    pub env: &'static str,
    pub address: String,
    pub routes_count: usize,
    pub workers: usize,
}

impl ServerBanner {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("┌─────────────────────────────────────────────────────────────┐\n");
        out.push_str("│ ⚡ AETHER WEB FRAMEWORK (Rust Async HTTP/1.1 Engine)        │\n");
        out.push_str("├─────────────────────────────────────────────────────────────┤\n");
        out.push_str(&format!("│ Environment : {:<44}│\n", self.env));
        out.push_str(&format!("│ Address     : {:<44}│\n", self.address));
        out.push_str(&format!("│ Routes      : {:<44}│\n", self.routes_count));
        out.push_str(&format!("│ Workers     : {:<44}│\n", format!("{} (configured; tokio work-stealing)", self.workers)));
        out.push_str("│ Allocator   : System allocator (per-request heap)           │\n");
        out.push_str("└─────────────────────────────────────────────────────────────┘\n");
        out
    }
}
