//! # CORS Middleware
//!
//! Implements the Fetch-standard CORS protocol: simple-response headers, preflight
//! validation, origin allow-listing, credentials, and cache correctness.

use aether_http::response::Response;

#[derive(Clone, Debug)]
pub struct CorsConfig {
    /// Origin advertised on responses. `"*"` allows any origin but is incompatible with
    /// `allow_credentials`, per the Fetch standard.
    pub allow_origin: &'static str,
    /// Exact origins to echo back. When this slice is non-empty it is authoritative and
    /// `allow_origin` is ignored: a request whose `Origin` is not listed receives no CORS
    /// headers at all. An allow-list that silently fell back to a wildcard would provide no
    /// protection, so deny-by-default is the only sound reading.
    pub allowed_origins: &'static [&'static str],
    pub allow_methods: &'static str,
    pub allow_headers: &'static str,
    /// Response headers a browser may expose to script (`Access-Control-Expose-Headers`).
    pub expose_headers: &'static str,
    /// Emit `Access-Control-Allow-Credentials: true`. Requires an explicit origin; the
    /// wildcard `*` is never valid with credentials.
    pub allow_credentials: bool,
    /// `Access-Control-Max-Age` advertised on preflight responses.
    pub max_age_secs: u32,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origin: "*",
            allowed_origins: &[],
            allow_methods: "GET, POST, PUT, DELETE, PATCH, OPTIONS",
            allow_headers: "Authorization, Content-Type, Accept",
            expose_headers: "",
            allow_credentials: false,
            max_age_secs: 86400,
        }
    }
}

pub struct CorsMiddleware {
    config: CorsConfig,
}

impl CorsMiddleware {
    pub fn new(config: CorsConfig) -> Self {
        Self { config }
    }

    /// Applies the static CORS headers to a response, without consulting the request's
    /// `Origin`. Retained for callers that only need the wildcard behaviour; `app` uses
    /// [`Self::apply_for_origin`] so an allow-listed origin can be reflected.
    pub fn apply(&self, response: &mut Response) {
        self.apply_for_origin(response, None);
    }

    /// Applies CORS headers for a simple (non-preflight) response.
    ///
    /// Emits `Vary: Origin` whenever the effective origin is not `*`, because the value
    /// depends on the request and a shared cache must not reuse it across origins
    /// (RFC 9111 §4.1). This is the header whose absence turns origin reflection into a
    /// cache-poisoning vulnerability.
    pub fn apply_for_origin(&self, response: &mut Response, origin: Option<&str>) {
        let Some(effective) = self.resolve_origin(origin) else {
            return;
        };

        set_header_once(response, "Access-Control-Allow-Origin", effective.to_string());
        if effective != "*" {
            add_vary_origin(response);
        }
        if !self.config.expose_headers.is_empty() {
            set_header_once(
                response,
                "Access-Control-Expose-Headers",
                self.config.expose_headers,
            );
        }
        if self.credentials_allowed(&effective) {
            set_header_once(response, "Access-Control-Allow-Credentials", "true".to_string());
        }
    }

    /// Answers a preflight request. Returns `true` when the requested method and headers are
    /// permitted; the caller should then send 204. Returns `false` when they are not, in
    /// which case no CORS headers are emitted and the caller should send 403.
    ///
    /// `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers` and
    /// `Access-Control-Max-Age` are preflight-only and therefore only appear here.
    pub fn apply_preflight(
        &self,
        response: &mut Response,
        origin: Option<&str>,
        requested_method: Option<&str>,
        requested_headers: Option<&str>,
    ) -> bool {
        let Some(effective) = self.resolve_origin(origin) else {
            return false;
        };

        if let Some(m) = requested_method {
            if !self.method_allowed(m) {
                return false;
            }
        }
        if let Some(hdrs) = requested_headers {
            if !self.headers_allowed(hdrs) {
                return false;
            }
        }

        set_header_once(response, "Access-Control-Allow-Origin", effective.to_string());
        if effective != "*" {
            add_vary_origin(response);
        }
        if self.credentials_allowed(&effective) {
            set_header_once(response, "Access-Control-Allow-Credentials", "true".to_string());
        }
        set_header_once(
            response,
            "Access-Control-Allow-Methods",
            self.config.allow_methods,
        );
        set_header_once(
            response,
            "Access-Control-Allow-Headers",
            self.config.allow_headers,
        );
        if self.config.max_age_secs > 0 {
            set_header_once(
                response,
                "Access-Control-Max-Age",
                self.config.max_age_secs.to_string(),
            );
        }
        true
    }

    /// The origin to advertise, or `None` when the request's origin is not allowed.
    fn resolve_origin(&self, origin: Option<&str>) -> Option<&str> {
        if self.config.allowed_origins.is_empty() {
            return Some(self.config.allow_origin);
        }
        // With an allow-list configured, an absent or unlisted Origin is denied outright.
        let request_origin = origin?;
        self.config
            .allowed_origins
            .iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(request_origin))
            .copied()
    }

    /// `Access-Control-Allow-Credentials` must never accompany `Access-Control-Allow-Origin: *`;
    /// browsers reject the pair, and emitting it would offer a false sense of security.
    fn credentials_allowed(&self, effective_origin: &str) -> bool {
        self.config.allow_credentials && effective_origin != "*"
    }

    fn method_allowed(&self, method: &str) -> bool {
        let allowed = self.config.allow_methods.trim();
        if allowed == "*" {
            return true;
        }
        allowed
            .split(',')
            .any(|m| m.trim().eq_ignore_ascii_case(method.trim()))
    }

    fn headers_allowed(&self, requested: &str) -> bool {
        if self.config.allow_headers.trim() == "*" {
            return true;
        }
        requested.split(',').all(|requested_header| {
            let requested_header = requested_header.trim();
            if requested_header.is_empty() {
                return true;
            }
            // Simple headers are always permitted without being listed.
            const SAFELISTED: [&str; 4] = ["accept", "accept-language", "content-language", "content-type"];
            if SAFELISTED.iter().any(|h| h.eq_ignore_ascii_case(requested_header)) {
                return true;
            }
            self.config
                .allow_headers
                .split(',')
                .any(|allowed| allowed.trim().eq_ignore_ascii_case(requested_header))
        })
    }
}

/// Sets a single-valued header, replacing any value the handler already put there.
/// `add_header` would push a second copy, and a response with two
/// `Access-Control-Allow-Origin` lines is rejected outright by browsers (the CORS spec
/// requires exactly one), so a handler that already manages its own CORS value must have
/// it overwritten rather than duplicated.
fn set_header_once(response: &mut Response, key: &'static str, value: impl Into<String>) {
    let value = value.into();
    if let Some((_, existing)) = response
        .headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        *existing = value;
    } else {
        response.add_header(key, value);
    }
}

fn add_vary_origin(response: &mut Response) {
    if let Some((_, existing)) = response
        .headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case("vary"))
    {
        let already = existing
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("origin"));
        if !already {
            existing.push_str(", Origin");
        }
    } else {
        response.add_header("Vary", "Origin");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(res: &Response, name: &str) -> Vec<String> {
        res.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .collect()
    }

    #[test]
    fn test_wildcard_default() {
        let mw = CorsMiddleware::new(CorsConfig::default());
        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("https://example.com"));

        assert_eq!(header(&res, "Access-Control-Allow-Origin"), vec!["*"]);
        // `*` does not vary, so no Vary is needed.
        assert!(header(&res, "Vary").is_empty());
        assert!(header(&res, "Access-Control-Allow-Credentials").is_empty());
    }

    /// A handler that already wrote a single-valued CORS header must end up with exactly
    /// one copy: browsers reject a response carrying two `Access-Control-Allow-Origin`
    /// lines, so `add_header`-style appending turns middleware + handler into a
    /// self-inflicted CORS failure.
    #[test]
    fn test_middleware_does_not_duplicate_handler_cors_headers() {
        let mw = CorsMiddleware::new(CorsConfig::default());
        let mut res = Response::ok();
        res.add_header("Access-Control-Allow-Origin", "https://custom.example");
        res.add_header("Access-Control-Allow-Credentials", "true");
        mw.apply_for_origin(&mut res, Some("https://example.com"));

        assert_eq!(header(&res, "Access-Control-Allow-Origin"), vec!["*"]);
        assert_eq!(header(&res, "Access-Control-Allow-Credentials"), vec!["true"]);
    }

    #[test]
    fn test_allowlist_reflects_matching_origin_and_varies() {
        let mw = CorsMiddleware::new(CorsConfig {
            allowed_origins: &["https://app.example.com", "https://admin.example.com"],
            ..CorsConfig::default()
        });

        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("https://app.example.com"));
        assert_eq!(
            header(&res, "Access-Control-Allow-Origin"),
            vec!["https://app.example.com"]
        );
        assert_eq!(header(&res, "Vary"), vec!["Origin"]);

        // Case-insensitive match, canonical form echoed.
        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("HTTPS://ADMIN.EXAMPLE.COM"));
        assert_eq!(
            header(&res, "Access-Control-Allow-Origin"),
            vec!["https://admin.example.com"]
        );
    }

    #[test]
    fn test_allowlist_rejects_unknown_origin() {
        let mw = CorsMiddleware::new(CorsConfig {
            allowed_origins: &["https://app.example.com"],
            ..CorsConfig::default()
        });

        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("https://evil.example.com"));
        assert!(header(&res, "Access-Control-Allow-Origin").is_empty());
    }

    /// `https://evil-app.example.com` must not match `https://app.example.com`.
    #[test]
    fn test_allowlist_is_not_a_substring_match() {
        let mw = CorsMiddleware::new(CorsConfig {
            allowed_origins: &["https://app.example.com"],
            ..CorsConfig::default()
        });

        for origin in [
            "https://app.example.com.evil.com",
            "https://evil.com/https://app.example.com",
            "http://app.example.com",
            "https://app.example.com:8443",
        ] {
            let mut res = Response::ok();
            mw.apply_for_origin(&mut res, Some(origin));
            assert!(
                header(&res, "Access-Control-Allow-Origin").is_empty(),
                "origin {origin} must not be allowed"
            );
        }
    }

    #[test]
    fn test_wildcard_plus_credentials_fails_closed() {
        let mw = CorsMiddleware::new(CorsConfig {
            allow_credentials: true,
            ..CorsConfig::default()
        });
        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("https://example.com"));

        assert_eq!(header(&res, "Access-Control-Allow-Origin"), vec!["*"]);
        // Never emit credentials alongside a wildcard: the browser rejects the pair.
        assert!(header(&res, "Access-Control-Allow-Credentials").is_empty());
    }

    #[test]
    fn test_credentials_with_explicit_origin() {
        let mw = CorsMiddleware::new(CorsConfig {
            allow_origin: "https://app.example.com",
            allow_credentials: true,
            ..CorsConfig::default()
        });
        let mut res = Response::ok();
        mw.apply_for_origin(&mut res, Some("https://app.example.com"));

        assert_eq!(
            header(&res, "Access-Control-Allow-Origin"),
            vec!["https://app.example.com"]
        );
        assert_eq!(header(&res, "Access-Control-Allow-Credentials"), vec!["true"]);
        assert_eq!(header(&res, "Vary"), vec!["Origin"]);
    }

    #[test]
    fn test_preflight_emits_max_age_and_methods() {
        let mw = CorsMiddleware::new(CorsConfig::default());
        let mut res = Response::status(204, "No Content");

        let allowed = mw.apply_preflight(&mut res, Some("https://example.com"), Some("POST"), Some("content-type"));
        assert!(allowed);
        assert_eq!(header(&res, "Access-Control-Max-Age"), vec!["86400"]);
        assert_eq!(
            header(&res, "Access-Control-Allow-Methods"),
            vec!["GET, POST, PUT, DELETE, PATCH, OPTIONS"]
        );
        assert!(header(&res, "Access-Control-Allow-Headers").len() == 1);
    }

    #[test]
    fn test_preflight_rejects_disallowed_method() {
        let mw = CorsMiddleware::new(CorsConfig {
            allow_methods: "GET, POST",
            ..CorsConfig::default()
        });
        let mut res = Response::status(204, "No Content");

        assert!(!mw.apply_preflight(&mut res, Some("https://example.com"), Some("DELETE"), None));
        assert!(header(&res, "Access-Control-Allow-Origin").is_empty());
    }

    #[test]
    fn test_preflight_rejects_disallowed_header() {
        let mw = CorsMiddleware::new(CorsConfig::default());
        let mut res = Response::status(204, "No Content");

        assert!(!mw.apply_preflight(
            &mut res,
            Some("https://example.com"),
            Some("GET"),
            Some("X-Custom-Secret")
        ));
        assert!(mw.apply_preflight(
            &mut res,
            Some("https://example.com"),
            Some("GET"),
            Some("Content-Type, Authorization")
        ));
    }

    #[test]
    fn test_preflight_allows_safelisted_headers() {
        let mw = CorsMiddleware::new(CorsConfig {
            allow_headers: "Authorization",
            ..CorsConfig::default()
        });
        let mut res = Response::status(204, "No Content");
        assert!(mw.apply_preflight(
            &mut res,
            Some("https://x.com"),
            Some("GET"),
            Some("accept, content-type, accept-language, content-language")
        ));
    }

    #[test]
    fn test_preflight_rejects_unknown_origin_against_allowlist() {
        let mw = CorsMiddleware::new(CorsConfig {
            allowed_origins: &["https://app.example.com"],
            ..CorsConfig::default()
        });
        let mut res = Response::status(204, "No Content");

        assert!(!mw.apply_preflight(&mut res, Some("https://evil.example.com"), Some("GET"), None));
        assert!(header(&res, "Access-Control-Allow-Origin").is_empty());
    }

    #[test]
    fn test_vary_origin_not_duplicated_and_composes() {
        let mw = CorsMiddleware::new(CorsConfig {
            allow_origin: "https://app.example.com",
            ..CorsConfig::default()
        });

        let mut res = Response::ok();
        res.add_header("Vary", "Accept-Encoding");
        mw.apply_for_origin(&mut res, Some("https://app.example.com"));
        mw.apply_for_origin(&mut res, Some("https://app.example.com"));

        let vary = header(&res, "Vary");
        assert_eq!(vary.len(), 1, "Vary duplicated: {:?}", vary);
        assert!(vary[0].contains("Accept-Encoding"));
        assert_eq!(vary[0].to_ascii_lowercase().matches("origin").count(), 1);
    }

    #[test]
    fn test_legacy_apply_still_works() {
        let mw = CorsMiddleware::new(CorsConfig::default());
        let mut res = Response::ok();
        mw.apply(&mut res);
        assert_eq!(header(&res, "Access-Control-Allow-Origin"), vec!["*"]);
        // Preflight-only headers must not leak onto simple responses.
        assert!(header(&res, "Access-Control-Allow-Methods").is_empty());
        assert!(header(&res, "Access-Control-Max-Age").is_empty());
    }
}
