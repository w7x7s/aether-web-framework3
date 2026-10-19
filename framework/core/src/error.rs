use crate::diagnostic::{Diagnostic, ErrorCode};
use std::fmt;

#[derive(Debug)]
pub enum AetherError {
    HeaderTooLarge,
    BodyLimitExceeded,
    MalformedRequest,
    RouteNotFound,
    MethodNotAllowed,
    ValidationFailed(String),
    DependencyCycleDetected(String),
    Timeout,
    Io(std::io::Error),
    Serialization(String),
    Custom(String),
}

impl AetherError {
    pub fn error_code(&self) -> ErrorCode {
        match self {
            AetherError::HeaderTooLarge | AetherError::BodyLimitExceeded => {
                ErrorCode::E3002PayloadLimitExceeded
            }
            AetherError::MalformedRequest => ErrorCode::E3001MalformedRequest,
            AetherError::RouteNotFound => ErrorCode::E2002RouteNotFound,
            AetherError::MethodNotAllowed => ErrorCode::E2003MethodNotAllowed,
            AetherError::ValidationFailed(_) => ErrorCode::E4002ValidationFailed,
            AetherError::DependencyCycleDetected(_) => ErrorCode::E5001DependencyCycle,
            AetherError::Timeout => ErrorCode::E5002InternalServerError,
            AetherError::Io(_) => ErrorCode::E1001ServerStartupFailed,
            AetherError::Serialization(_) => ErrorCode::E4001DeserializationFailed,
            AetherError::Custom(_) => ErrorCode::E5002InternalServerError,
        }
    }

    pub fn to_diagnostic(&self) -> Diagnostic {
        let code = self.error_code();
        let reason = self.to_string();
        let diag = Diagnostic::new(code, reason);

        match self {
            AetherError::Io(_err) => diag
                .with_hint("Check if the network address/port is already occupied by another service.")
                .with_suggestion("Change the listening port via AETHER_PORT environment variable.")
                .with_suggestion("Stop conflicting background processes listening on the port."),
            AetherError::MalformedRequest => diag
                .with_hint("The request violates RFC 9112 HTTP grammar or contains illegal headers.")
                .with_suggestion("Ensure HTTP clients send valid HTTP/1.1 or HTTP/2 frames."),
            AetherError::BodyLimitExceeded => diag
                .with_hint("Payload size exceeded the configured max_body_bytes limit.")
                .with_suggestion("Increase ServerLimits::max_body_bytes if uploading large files."),
            AetherError::Serialization(msg) => diag
                .with_context("Field Validation", msg.clone())
                .with_hint("JSON structure does not match the target Rust struct DTO.")
                .with_suggestion("Inspect client payload keys and expected data types."),
            _ => diag,
        }
    }
}

impl fmt::Display for AetherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AetherError::HeaderTooLarge => {
                write!(f, "HTTP Header section exceeded maximum allowed budget")
            }
            AetherError::BodyLimitExceeded => write!(f, "Request body exceeded payload limit"),
            AetherError::MalformedRequest => write!(f, "Malformed HTTP request syntax"),
            AetherError::RouteNotFound => write!(f, "Route not found"),
            AetherError::MethodNotAllowed => write!(f, "HTTP method not allowed for route"),
            AetherError::ValidationFailed(msg) => write!(f, "Validation failure: {}", msg),
            AetherError::DependencyCycleDetected(msg) => {
                write!(f, "Circular dependency in DI graph: {}", msg)
            }
            AetherError::Timeout => write!(f, "Connection timeout"),
            AetherError::Io(err) => write!(f, "I/O Error: {}", err),
            AetherError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            AetherError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AetherError {}

impl From<std::io::Error> for AetherError {
    fn from(err: std::io::Error) -> Self {
        AetherError::Io(err)
    }
}
