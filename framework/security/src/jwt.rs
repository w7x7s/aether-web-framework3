//! # JWT Validator
//!
//! HMAC-SHA256 signature verification with constant-time comparison via `ring`, plus
//! optional standard-claim validation (`alg`, `exp`, `nbf`, `iat`, `iss`, `aud`).

use aether_core::error::AetherError;
use ring::hmac;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct JwtValidator {
    key: hmac::Key,
}

/// Claim checks applied by [`JwtValidator::verify`].
#[derive(Debug, Clone)]
pub struct JwtValidationOptions {
    /// Algorithm the token header must declare. Tokens signed with a different algorithm
    /// (including `none`) are rejected even if the HMAC would otherwise verify.
    pub expected_alg: &'static str,
    /// If set, the `iss` claim must match exactly.
    pub issuer: Option<String>,
    /// If set, the `aud` claim (string or array) must contain this audience.
    pub audience: Option<String>,
    /// Clock skew tolerated when checking `exp`, `nbf`, and future `iat` values.
    pub leeway_secs: u64,
    /// Fixed clock for deterministic tests. `None` uses the system clock.
    pub now_unix: Option<u64>,
}

impl Default for JwtValidationOptions {
    fn default() -> Self {
        Self {
            expected_alg: "HS256",
            issuer: None,
            audience: None,
            leeway_secs: 60,
            now_unix: None,
        }
    }
}

/// A token whose signature and requested claims have been validated.
#[derive(Debug)]
pub struct VerifiedJwt<'a> {
    pub header: &'a str,
    pub payload: &'a str,
    pub claims: serde_json::Value,
}

impl JwtValidator {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret),
        }
    }

    /// Verifies the token signature and returns the raw header and payload segments.
    pub fn verify_signature<'a>(&self, token: &'a str) -> Result<(&'a str, &'a str), AetherError> {
        let mut parts = token.split('.');
        let header = parts.next().ok_or(AetherError::MalformedRequest)?;
        let payload = parts.next().ok_or(AetherError::MalformedRequest)?;
        let sig = parts.next().ok_or(AetherError::MalformedRequest)?;

        if parts.next().is_some() {
            return Err(AetherError::MalformedRequest);
        }

        if header.is_empty() || payload.is_empty() || sig.is_empty() {
            return Err(AetherError::ValidationFailed("Empty token component".into()));
        }

        let sig_bytes = decode_base64url(sig)
            .map_err(|_| AetherError::ValidationFailed("Invalid base64url signature".into()))?;

        let signed_len = header.len() + 1 + payload.len();
        if signed_len > token.len() {
            return Err(AetherError::MalformedRequest);
        }
        let signed_data = token[..signed_len].as_bytes();

        // `ring` performs the comparison in constant time over the full tag.
        hmac::verify(&self.key, signed_data, &sig_bytes)
            .map_err(|_| AetherError::ValidationFailed("Signature verification failed".into()))?;

        Ok((header, payload))
    }

    /// Verifies the signature and the standard claims named in `options`.
    ///
    /// `exp` and `nbf` are only enforced when present; supply [`JwtValidationOptions`] with
    /// an issuer/audience when the tokens are scoped.
    pub fn verify<'a>(
        &self,
        token: &'a str,
        options: &JwtValidationOptions,
    ) -> Result<VerifiedJwt<'a>, AetherError> {
        let (header_segment, payload_segment) = self.verify_signature(token)?;

        let header_bytes = decode_base64url(header_segment)
            .map_err(|_| AetherError::ValidationFailed("Invalid header encoding".into()))?;
        let header: serde_json::Value = serde_json::from_slice(&header_bytes)
            .map_err(|_| AetherError::ValidationFailed("Header is not valid JSON".into()))?;
        let alg = header
            .get("alg")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::ValidationFailed("Missing alg header".into()))?;
        if alg != options.expected_alg {
            return Err(AetherError::ValidationFailed(format!(
                "Unexpected alg '{alg}', expected '{}'",
                options.expected_alg
            )));
        }

        let payload_bytes = decode_base64url(payload_segment)
            .map_err(|_| AetherError::ValidationFailed("Invalid payload encoding".into()))?;
        let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|_| AetherError::ValidationFailed("Payload is not valid JSON".into()))?;
        if !claims.is_object() {
            return Err(AetherError::ValidationFailed("Payload must be a JSON object".into()));
        }

        let now = options.now_unix.unwrap_or_else(current_unix_time);
        let leeway = options.leeway_secs;

        if let Some(exp) = numeric_claim(&claims, "exp")? {
            if now > exp.saturating_add(leeway) {
                return Err(AetherError::ValidationFailed("Token expired".into()));
            }
        }
        if let Some(nbf) = numeric_claim(&claims, "nbf")? {
            if now.saturating_add(leeway) < nbf {
                return Err(AetherError::ValidationFailed("Token not yet valid".into()));
            }
        }
        if let Some(iat) = numeric_claim(&claims, "iat")? {
            if iat > now.saturating_add(leeway) {
                return Err(AetherError::ValidationFailed(
                    "Token issued in the future".into(),
                ));
            }
        }
        if let Some(expected_issuer) = &options.issuer {
            let issuer = claims
                .get("iss")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AetherError::ValidationFailed("Missing iss claim".into()))?;
            if issuer != expected_issuer {
                return Err(AetherError::ValidationFailed("Issuer mismatch".into()));
            }
        }
        if let Some(expected_audience) = &options.audience {
            let matches = match claims.get("aud") {
                Some(serde_json::Value::String(aud)) => aud == expected_audience,
                Some(serde_json::Value::Array(audiences)) => audiences
                    .iter()
                    .any(|aud| aud.as_str() == Some(expected_audience.as_str())),
                _ => false,
            };
            if !matches {
                return Err(AetherError::ValidationFailed("Audience mismatch".into()));
            }
        }

        Ok(VerifiedJwt {
            header: header_segment,
            payload: payload_segment,
            claims,
        })
    }
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn numeric_claim(claims: &serde_json::Value, name: &str) -> Result<Option<u64>, AetherError> {
    match claims.get(name) {
        None => Ok(None),
        Some(value) => {
            let number = value.as_f64().ok_or_else(|| {
                AetherError::ValidationFailed(format!("Claim '{name}' must be a number"))
            })?;
            if !number.is_finite() || number < 0.0 {
                return Err(AetherError::ValidationFailed(format!(
                    "Claim '{name}' must be a non-negative number"
                )));
            }
            Ok(Some(number as u64))
        }
    }
}

/// Decodes standard base64url (RFC 4648 §5) into raw bytes.
fn decode_base64url(input: &str) -> Result<Vec<u8>, ()> {
    fn decode_char(c: u8) -> Result<u8, ()> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'-' => Ok(62),
            b'_' => Ok(63),
            _ => Err(()),
        }
    }

    let input_bytes = input.trim_end_matches('=').as_bytes();
    if input_bytes.len() % 4 == 1 {
        return Err(());
    }
    let mut out = Vec::with_capacity((input_bytes.len() * 3) / 4);

    let mut buf = 0u32;
    let mut bits = 0u32;

    for &b in input_bytes {
        let val = decode_char(b)?;
        buf = (buf << 6) | (val as u32);
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    // Non-zero trailing bits are a non-canonical encoding (and enable signature aliasing).
    if buf != 0 {
        return Err(());
    }

    Ok(out)
}

/// Encodes raw bytes to base64url without padding.
pub fn encode_base64url(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((input.len() * 4) / 3 + 4);

    let mut buf = 0u32;
    let mut bits = 0u32;

    for &b in input {
        buf = (buf << 8) | (b as u32);
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            let idx = (buf >> bits) & 0x3F;
            out.push(TABLE[idx as usize] as char);
            buf &= (1 << bits) - 1;
        }
    }

    if bits > 0 {
        let idx = (buf << (6 - bits)) & 0x3F;
        out.push(TABLE[idx as usize] as char);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constant_time::constant_time_eq;

    const SECRET: &[u8] = b"super-secret-key-1234567890-aether";

    fn sign(validator: &JwtValidator, header: &str, payload: &str) -> String {
        let signed = format!("{header}.{payload}");
        let tag = hmac::sign(&validator.key, signed.as_bytes());
        format!("{signed}.{}", encode_base64url(tag.as_ref()))
    }

    fn token(header_json: &str, payload_json: &str) -> String {
        let validator = JwtValidator::new(SECRET);
        sign(
            &validator,
            &encode_base64url(header_json.as_bytes()),
            &encode_base64url(payload_json.as_bytes()),
        )
    }

    #[test]
    fn test_jwt_verification_valid_and_invalid() {
        let validator = JwtValidator::new(SECRET);
        let header = encode_base64url(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = encode_base64url(br#"{"sub":"1234567890","name":"Hamza"}"#);
        let valid_token = sign(&validator, &header, &payload);

        let result = validator.verify_signature(&valid_token);
        assert!(result.is_ok());
        let (h, p) = result.unwrap();
        assert_eq!(h, header);
        assert_eq!(p, payload);

        let tampered_payload = encode_base64url(br#"{"sub":"1234567890","name":"Hacker"}"#);
        let tampered = format!("{header}.{tampered_payload}.{}", valid_token.rsplit('.').next().unwrap());
        assert!(validator.verify_signature(&tampered).is_err());

        let fake = format!("{header}.{payload}.fakeSignature123");
        assert!(validator.verify_signature(&fake).is_err());
    }

    #[test]
    fn test_claims_are_validated() {
        let validator = JwtValidator::new(SECRET);
        let options = JwtValidationOptions {
            issuer: Some("aether".into()),
            audience: Some("api".into()),
            now_unix: Some(1_000),
            ..JwtValidationOptions::default()
        };

        let good = token(
            r#"{"alg":"HS256"}"#,
            r#"{"iss":"aether","aud":["web","api"],"exp":1100,"nbf":900,"iat":900}"#,
        );
        let verified = validator.verify(&good, &options).unwrap();
        assert_eq!(verified.claims["iss"], "aether");

        let expired = token(r#"{"alg":"HS256"}"#, r#"{"iss":"aether","aud":"api","exp":900}"#);
        assert!(matches!(
            validator.verify(&expired, &options),
            Err(AetherError::ValidationFailed(_))
        ));

        let not_yet = token(r#"{"alg":"HS256"}"#, r#"{"iss":"aether","aud":"api","nbf":1200}"#);
        assert!(validator.verify(&not_yet, &options).is_err());

        let wrong_iss = token(r#"{"alg":"HS256"}"#, r#"{"iss":"evil","aud":"api"}"#);
        assert!(validator.verify(&wrong_iss, &options).is_err());

        let wrong_aud = token(r#"{"alg":"HS256"}"#, r#"{"iss":"aether","aud":"other"}"#);
        assert!(validator.verify(&wrong_aud, &options).is_err());

        let future_iat = token(
            r#"{"alg":"HS256"}"#,
            r#"{"iss":"aether","aud":"api","iat":9999}"#,
        );
        assert!(validator.verify(&future_iat, &options).is_err());
    }

    #[test]
    fn test_algorithm_confusion_is_rejected() {
        let validator = JwtValidator::new(SECRET);
        let options = JwtValidationOptions {
            now_unix: Some(1_000),
            ..JwtValidationOptions::default()
        };

        // Correctly HMAC-signed, but the header claims a different algorithm.
        for alg in ["none", "RS256", "HS512"] {
            let header = format!(r#"{{"alg":"{alg}"}}"#);
            let token = token(&header, r#"{"sub":"1"}"#);
            assert!(
                validator.verify(&token, &options).is_err(),
                "alg={alg} must be rejected"
            );
        }
    }

    #[test]
    fn test_malformed_claims_rejected() {
        let validator = JwtValidator::new(SECRET);
        let options = JwtValidationOptions {
            now_unix: Some(1_000),
            ..JwtValidationOptions::default()
        };

        for payload in [
            "not json",
            r#"["array"]"#,
            r#"{"exp":"soon"}"#,
            r#"{"exp":-1}"#,
        ] {
            let token = token(r#"{"alg":"HS256"}"#, payload);
            assert!(
                validator.verify(&token, &options).is_err(),
                "payload {payload} must be rejected"
            );
        }
    }

    #[test]
    fn test_base64url_rejects_non_canonical_encodings() {
        // 'A' is zero bits; a single character cannot encode a byte.
        assert!(decode_base64url("A").is_err());
        // One byte 0xFF is "_w"; "_x" has non-zero trailing bits.
        assert_eq!(decode_base64url("_w").unwrap(), vec![0xFF]);
        assert!(decode_base64url("_x").is_err());
        // Standard base64 alphabet is not accepted.
        assert!(decode_base64url("+/").is_err());
    }

    #[test]
    fn test_constant_time_helper_still_available() {
        assert!(constant_time_eq(b"a", b"a"));
        assert!(!constant_time_eq(b"a", b"b"));
    }
}
