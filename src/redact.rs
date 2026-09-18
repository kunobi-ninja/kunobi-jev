//! Header redaction for debug logs.

use reqwest::header::HeaderMap;

/// Credential headers that keep a short key suffix for identification.
const KEY_HEADERS: [&str; 3] = ["authorization", "proxy-authorization", "x-api-key"];

/// Headers whose values are hidden in full.
const OPAQUE_HEADERS: [&str; 2] = ["cookie", "set-cookie"];

/// Copy headers as text with known credential values redacted.
pub(crate) fn redact_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str();
            let value = match value.to_str() {
                Ok(text) => redact(name, text),
                Err(_) => "<non-utf8>".to_owned(),
            };
            (name.to_owned(), value)
        })
        .collect()
}

/// `name` must already be lowercase, as `HeaderName` guarantees.
fn redact(name: &str, value: &str) -> String {
    // The official SDKs also redact "any header whose name contains `token` or
    // `secret`", which catches a caller's own credential headers.
    if KEY_HEADERS.contains(&name) || name.contains("token") || name.contains("secret") {
        redact_key(value)
    } else if OPAQUE_HEADERS.contains(&name) {
        "***".to_owned()
    } else {
        value.to_owned()
    }
}

/// Mask a key, keeping its scheme and the last four characters of secrets longer than eight.
fn redact_key(value: &str) -> String {
    let (scheme, secret) = match value.find(char::is_whitespace) {
        Some(split) => {
            let rest = value[split..].trim_start();
            let secret = rest.split(char::is_whitespace).next().unwrap_or("");
            (Some(&value[..split]), secret)
        }
        None => (None, value),
    };
    let chars = secret.chars().count();
    let tail: String = if chars > 8 {
        secret.chars().skip(chars - 4).collect()
    } else {
        String::new()
    };
    match scheme {
        Some(scheme) if !scheme.is_empty() => format!("{scheme} ***{tail}"),
        _ => format!("***{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn masks_keys_but_keeps_scheme_and_suffix() {
        assert_eq!(redact_key("Bearer sk-1234567890abcd"), "Bearer ***abcd");
        assert_eq!(redact_key("Bearer short"), "Bearer ***");
        assert_eq!(redact_key("sk-1234567890abcd"), "***abcd");
        assert_eq!(redact_key("12345678"), "***");
        assert_eq!(
            redact_key("Bearer   spaced-secret-value extra"),
            "Bearer ***alue"
        );
        assert_eq!(redact_key(" leading-space-key"), "***-key");
        assert_eq!(redact_key(""), "***");
    }

    #[test]
    fn redacts_only_credential_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer sk-1234567890abcd"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("abc"));
        headers.insert("cookie", HeaderValue::from_static("session=secret"));
        headers.insert("accept", HeaderValue::from_static("application/json"));
        let redacted = redact_headers(&headers);
        let get = |name: &str| {
            redacted
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        assert_eq!(get("authorization"), "Bearer ***abcd");
        assert_eq!(get("x-api-key"), "***");
        assert_eq!(get("cookie"), "***");
        assert_eq!(get("accept"), "application/json");

        // A caller's own credential header is redacted by name.
        let mut caller = HeaderMap::new();
        caller.insert(
            "x-tenant-token",
            HeaderValue::from_static("tok-abcdefghijkl"),
        );
        caller.insert("client-secret", HeaderValue::from_static("shhhhhhhhhhh"));
        caller.insert("x-request-tokens", HeaderValue::from_static("42"));
        let redacted = redact_headers(&caller);
        let value = |name: &str| {
            redacted
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
                .unwrap()
                .to_owned()
        };
        assert_eq!(value("x-tenant-token"), "***ijkl");
        assert_eq!(value("client-secret"), "***hhhh");
        // Including a counter that merely mentions tokens: better a redacted
        // number than a leaked key.
        assert_eq!(value("x-request-tokens"), "***");
    }
}
