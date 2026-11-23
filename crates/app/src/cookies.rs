//! Reading the interface's own cookies (theme and language) from a request.

use axum::http::{HeaderMap, header};

/// The values of the cookie `name` across all `Cookie` headers, in order. A browser can send the
/// same name twice (another path or domain), so callers take the last valid one.
pub fn values<'a>(
    headers: &'a HeaderMap,
    name: &'a str,
) -> impl DoubleEndedIterator<Item = &'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(move |(cookie, _)| *cookie == name)
        .map(|(_, value)| value)
}
