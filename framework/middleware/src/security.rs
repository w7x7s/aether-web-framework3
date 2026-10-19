use aether_http::response::Response;

/// Production security headers middleware.
///
/// Defaults follow current OWASP and MDN guidance. Two deliberate notes:
///
/// * `xss_protection` is `"0"`, not `"1; mode=block"`. The legacy XSS auditor is removed from
///   every current browser and, while it existed, it was itself a source of cross-origin
///   information leaks and bypasses. Sending `0` disables it deterministically; sending
///   `1; mode=block` re-enables a known-harmful code path in the few clients that still
///   honour it.
/// * `content_security_policy` and `permissions_policy` default to `None`. Unlike the other
///   headers here, a wrong value for these breaks the application rather than hardening it,
///   so the framework refuses to guess.
///
/// Every header is applied with "set unless already present", so a handler that sets its own
/// `X-Frame-Options` or `Content-Security-Policy` wins over the middleware default instead of
/// producing a duplicate header of ambiguous meaning.
#[derive(Debug, Clone)]
pub struct SecurityHeadersMiddleware {
    pub hsts: bool,
    pub frame_options: &'static str,
    pub content_type_options: &'static str,
    pub xss_protection: &'static str,
    pub referrer_policy: Option<&'static str>,
    pub permissions_policy: Option<&'static str>,
    pub content_security_policy: Option<&'static str>,
    pub cross_origin_opener_policy: Option<&'static str>,
    pub cross_origin_resource_policy: Option<&'static str>,
}

impl Default for SecurityHeadersMiddleware {
    fn default() -> Self {
        Self {
            hsts: true,
            frame_options: "DENY",
            content_type_options: "nosniff",
            xss_protection: "0",
            // The browser default, stated explicitly so it cannot be changed by a
            // user-agent update or a proxy.
            referrer_policy: Some("strict-origin-when-cross-origin"),
            permissions_policy: None,
            content_security_policy: None,
            cross_origin_opener_policy: None,
            cross_origin_resource_policy: None,
        }
    }
}

impl SecurityHeadersMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply security headers to the outgoing response
    pub fn apply(&self, response: &mut Response) {
        set_unless_present(response, "X-Content-Type-Options", self.content_type_options);
        set_unless_present(response, "X-Frame-Options", self.frame_options);
        set_unless_present(response, "X-XSS-Protection", self.xss_protection);

        if let Some(v) = self.referrer_policy {
            set_unless_present(response, "Referrer-Policy", v);
        }
        if let Some(v) = self.permissions_policy {
            set_unless_present(response, "Permissions-Policy", v);
        }
        if let Some(v) = self.content_security_policy {
            set_unless_present(response, "Content-Security-Policy", v);
        }
        if let Some(v) = self.cross_origin_opener_policy {
            set_unless_present(response, "Cross-Origin-Opener-Policy", v);
        }
        if let Some(v) = self.cross_origin_resource_policy {
            set_unless_present(response, "Cross-Origin-Resource-Policy", v);
        }

        if self.hsts {
            set_unless_present(
                response,
                "Strict-Transport-Security",
                "max-age=31536000; includeSubDomains",
            );
        }
    }
}

fn set_unless_present(response: &mut Response, key: &'static str, value: &'static str) {
    if response
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(key))
    {
        return;
    }
    response.add_header(key, value);
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
    fn test_defaults_match_current_guidance() {
        let mw = SecurityHeadersMiddleware::new();
        // Regression: "1; mode=block" re-enables a removed, known-harmful XSS auditor.
        assert_eq!(mw.xss_protection, "0");
        assert_eq!(mw.content_type_options, "nosniff");
        assert_eq!(mw.frame_options, "DENY");
        assert!(mw.hsts);
    }

    #[test]
    fn test_apply_sets_baseline_headers_once() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        assert_eq!(header(&res, "X-Content-Type-Options"), vec!["nosniff"]);
        assert_eq!(header(&res, "X-Frame-Options"), vec!["DENY"]);
        assert_eq!(header(&res, "X-XSS-Protection"), vec!["0"]);
        assert_eq!(
            header(&res, "Strict-Transport-Security"),
            vec!["max-age=31536000; includeSubDomains"]
        );
        assert_eq!(
            header(&res, "Referrer-Policy"),
            vec!["strict-origin-when-cross-origin"]
        );
    }

    #[test]
    fn test_handler_supplied_header_is_not_duplicated() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        res.add_header("X-Frame-Options", "SAMEORIGIN");
        res.add_header("x-content-type-options", "nosniff");

        mw.apply(&mut res);
        mw.apply(&mut res);

        assert_eq!(header(&res, "X-Frame-Options"), vec!["SAMEORIGIN"]);
        assert_eq!(header(&res, "x-content-type-options"), vec!["nosniff"]);
    }

    #[test]
    fn test_optional_headers_are_off_by_default() {
        let mw = SecurityHeadersMiddleware::new();
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        // A guessed CSP or Permissions-Policy breaks applications, so the framework must not
        // emit one unless the owner configured it.
        assert!(header(&res, "Content-Security-Policy").is_empty());
        assert!(header(&res, "Permissions-Policy").is_empty());
        assert!(header(&res, "Cross-Origin-Opener-Policy").is_empty());
        assert!(header(&res, "Cross-Origin-Resource-Policy").is_empty());
    }

    #[test]
    fn test_configured_optional_headers_are_applied() {
        let mw = SecurityHeadersMiddleware {
            content_security_policy: Some("default-src 'self'"),
            permissions_policy: Some("geolocation=()"),
            cross_origin_resource_policy: Some("same-origin"),
            hsts: false,
            ..SecurityHeadersMiddleware::default()
        };
        let mut res = Response::text("hi");
        mw.apply(&mut res);

        assert_eq!(header(&res, "Content-Security-Policy"), vec!["default-src 'self'"]);
        assert_eq!(header(&res, "Permissions-Policy"), vec!["geolocation=()"]);
        assert_eq!(header(&res, "Cross-Origin-Resource-Policy"), vec!["same-origin"]);
        assert!(header(&res, "Strict-Transport-Security").is_empty());
    }
}
