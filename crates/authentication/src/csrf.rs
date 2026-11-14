//! Cross-site request forgery protection for cookie-authenticated requests. The server applies
//! [`allowed`] to every request, with a refusal page of its own.
//!
//! Session cookies are `SameSite=Lax`, so browsers do not send them with cross-site `POST`
//! requests. On top of that, state-changing requests are refused when the browser says they
//! come from another site (`Sec-Fetch-Site`) or from another origin (`Origin`). An
//! `Authorization` header exempts nothing: recto has no API authenticated that way, and a
//! browser can attach cached credentials (HTTP Basic from a reverse proxy) to a forged request.
//!
//! The check is deliberately lenient with requests that carry neither header: those come from
//! non-browser clients, which hold no session cookie of the user's, or from old browsers on
//! same-origin forms.

use std::sync::Arc;

use axum::http::{HeaderMap, Method, header};

/// The origin (`scheme://host[:port]`) the interface is served from.
#[derive(Debug, Clone)]
pub struct AllowedOrigin(Arc<str>);

impl AllowedOrigin {
    /// The origin of the instance's public URL; its path, if any, is ignored.
    pub fn new(public_url: &url::Url) -> Self {
        Self(public_url.origin().ascii_serialization().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Whether a request may proceed. Safe methods always may; others are refused when
/// `Sec-Fetch-Site` is present and not `same-origin` (or `none`, a navigation typed by the
/// user), or when `Origin` is present and differs from `origin`.
pub fn allowed(method: &Method, headers: &HeaderMap, origin: &AllowedOrigin) -> bool {
    if is_safe(method) {
        return true;
    }
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        && !matches!(site, "same-origin" | "none")
    {
        return false;
    }
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        Some(sent) => sent == &*origin.0,
        // Browsers send Origin with every cross-origin POST; requests without it come from
        // non-browser clients or same-origin navigations of older browsers.
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        headers
    }

    fn origin() -> AllowedOrigin {
        AllowedOrigin::new(&"https://recto.example.org/app/".parse().unwrap())
    }

    #[test]
    fn same_origin_post_is_allowed() {
        let same = headers(&[
            ("origin", "https://recto.example.org"),
            ("sec-fetch-site", "same-origin"),
        ]);
        assert!(allowed(&Method::POST, &same, &origin()));
        assert!(allowed(&Method::POST, &headers(&[]), &origin()));
    }

    #[test]
    fn cross_site_post_is_refused() {
        let origin = origin();
        for pairs in [
            &[("origin", "https://evil.example")][..],
            &[("sec-fetch-site", "cross-site")],
            &[("origin", "null")],
        ] {
            assert!(
                !allowed(&Method::POST, &headers(pairs), &origin),
                "{pairs:?}"
            );
        }
        assert!(!allowed(
            &Method::DELETE,
            &headers(&[("sec-fetch-site", "same-site")]),
            &origin
        ));
    }

    #[test]
    fn authorization_header_does_not_exempt_cross_site_posts() {
        let forged = headers(&[
            ("authorization", "Basic dXNlcjpwYXNz"),
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert!(!allowed(&Method::POST, &forged, &origin()));
    }

    #[test]
    fn get_is_always_allowed() {
        let hostile = headers(&[
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
        ]);
        assert!(allowed(&Method::GET, &hostile, &origin()));
    }
}
