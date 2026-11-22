//! How computers and phones authenticate: with a device password, sent as the password of HTTP
//! Basic (the username is ignored, since clients fill in the computer's login name) or as the
//! secret of a personal address, `/device/<secret>/...`, for clients that cannot answer a
//! password prompt. The account password is never accepted here, and a device password is never
//! accepted anywhere else: it cannot sign in to the web pages.
//!
//! Failed lookups have their own limiter, per TCP peer address: a burst of
//! [`DeviceLimiter::BURST`], then one per second. The secret has 160 random bits, so slowing
//! guesses gains nothing; the limiter only keeps floods off the database. It is not the sign-in
//! limiter on purpose: behind a reverse proxy or a NAT every client shares one address, and one
//! phone polling with a revoked password would otherwise lock everyone out of password sign-in.
//! Successful requests never count, so a device that polls every few seconds is never limited.
//!
//! The secret never reaches a log line: refusals log the peer, and the routes log the device
//! password's id and name.

use std::{
    borrow::Cow,
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use axum::{
    extract::ConnectInfo,
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use data_encoding::BASE64;
use recto_domain::device_passwords::{self, DeviceUser};
use sea_orm::DatabaseConnection;

use crate::state::AuthState;

/// Where a request's device secret was.
pub enum Secret<'a> {
    /// The password of an `Authorization: Basic` header.
    Basic(String),
    /// The `<secret>` of a personal address.
    Path(&'a str),
    Missing,
}

/// The secret a request carries: the personal address's when the route has one, else the
/// password of its Basic credentials. Credentials that cannot be read count as missing.
pub fn secret<'a>(parts: &Parts, path_secret: Option<&'a str>) -> Secret<'a> {
    if let Some(secret) = path_secret {
        return Secret::Path(secret);
    }
    let password = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, credentials) = value.trim().split_once(' ')?;
            scheme.eq_ignore_ascii_case("basic").then_some(credentials)
        })
        .and_then(|credentials| BASE64.decode(credentials.trim().as_bytes()).ok())
        .and_then(|decoded| String::from_utf8(decoded).ok())
        .and_then(|text| {
            let (_username, password) = text.split_once(':')?;
            Some(password.to_owned())
        });
    match password {
        Some(password) => Secret::Basic(password),
        None => Secret::Missing,
    }
}

/// Why a device was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No credentials: 401, so the client asks for them.
    Missing,
    /// An unknown, revoked or malformed secret, or one of a disabled user. A personal address
    /// that does not exist is 404, like any other unknown address.
    Wrong { in_path: bool },
    /// Too many failures from this address: 429.
    TooMany,
    /// Credentials over plain HTTP, which recto does not accept unless told to: 403.
    Plain,
    /// The database failed: 503.
    Unavailable,
}

/// The address a device connects from: the TCP peer, never a forwarded header, which a client
/// could write itself. An IPv4 address seen as IPv6 (`::ffff:a.b.c.d`, from a dual-stack
/// socket) is the IPv4 address.
pub fn peer(parts: &Parts) -> IpAddr {
    match parts.extensions.get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(address)) => address.ip().to_canonical(),
        None => {
            // The server always knows its peers; only a router served without connection
            // information lands here.
            tracing::warn!("the address of a device is unknown");
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        }
    }
}

/// Failed device authentications per client address: a token bucket of [`Self::BURST`]
/// failures, refilled at one per second. Checking does not spend; only a failure does.
#[derive(Clone, Default)]
pub struct DeviceLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, Bucket>>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn tokens_at(self, now: Instant) -> f64 {
        let refilled = now.saturating_duration_since(self.at).as_secs_f64() * DeviceLimiter::RATE;
        (self.tokens + refilled).min(DeviceLimiter::BURST)
    }
}

impl DeviceLimiter {
    /// The failures a client may have in a row.
    pub const BURST: f64 = 30.0;
    /// The failures a client gets back per second.
    const RATE: f64 = 1.0;
    /// Past this many clients with failures, the ones whose budget is full again are forgotten.
    const CLIENTS: usize = 10_000;

    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `peer` may try a secret now.
    pub fn allow(&self, peer: IpAddr) -> bool {
        self.allow_at(peer, Instant::now())
    }

    /// Counts a failed attempt of `peer`.
    pub fn failed(&self, peer: IpAddr) {
        self.failed_at(peer, Instant::now());
    }

    fn allow_at(&self, peer: IpAddr, now: Instant) -> bool {
        let buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        buckets
            .get(&counted_address(peer))
            .is_none_or(|bucket| bucket.tokens_at(now) >= 1.0)
    }

    fn failed_at(&self, peer: IpAddr, now: Instant) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() >= Self::CLIENTS {
            buckets.retain(|_, bucket| bucket.tokens_at(now) < Self::BURST);
        }
        let bucket = buckets.entry(counted_address(peer)).or_insert(Bucket {
            tokens: Self::BURST,
            at: now,
        });
        *bucket = Bucket {
            tokens: (bucket.tokens_at(now) - 1.0).max(0.0),
            at: now,
        };
    }
}

/// The address failures are counted by. An IPv6 client usually has a whole /64 to pick
/// addresses from, so it counts by that network, as password sign-in does.
fn counted_address(address: IpAddr) -> IpAddr {
    match address.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        v4 => v4,
    }
}

/// `path` as it may be logged: the secret of a personal address, `/device/<secret>/...`, is
/// replaced by `<secret>`.
pub fn loggable_path(path: &str) -> Cow<'_, str> {
    let Some(rest) = path.strip_prefix(PERSONAL_PREFIX) else {
        return Cow::Borrowed(path);
    };
    match rest.find('/').map_or(rest, |end| &rest[..end]) {
        "" => Cow::Borrowed(path),
        secret => Cow::Owned(format!(
            "{PERSONAL_PREFIX}<secret>{}",
            &rest[secret.len()..]
        )),
    }
}

/// Where personal addresses start.
pub const PERSONAL_PREFIX: &str = "/device/";

/// What device authentication needs.
#[derive(Clone)]
pub struct DeviceAuth {
    pub database: DatabaseConnection,
    pub limiter: DeviceLimiter,
    /// Whether credentials are accepted over plain HTTP: the public URL is `https` (the proxy
    /// in front terminates TLS), or `RECTO_IPP_ALLOW_PLAIN` is set. Loopback peers always are.
    pub plain_allowed: bool,
    /// The realm of the password prompt.
    pub brand_name: Arc<str>,
}

impl DeviceAuth {
    /// Device authentication for the server of `auth`, with a limiter of its own.
    pub fn new(auth: &AuthState, brand_name: &str) -> Self {
        Self {
            database: auth.database.clone(),
            limiter: DeviceLimiter::new(),
            plain_allowed: auth.public_url.scheme() == "https" || auth.ipp_allow_plain,
            brand_name: Arc::from(brand_name),
        }
    }

    /// The answer to a refused device: see [`Refusal`]. The bodies are empty, since devices
    /// only read the status.
    pub fn refuse(&self, refusal: Refusal) -> Response {
        match refusal {
            Refusal::Missing | Refusal::Wrong { in_path: false } => (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, self.challenge())],
            )
                .into_response(),
            Refusal::Wrong { in_path: true } => StatusCode::NOT_FOUND.into_response(),
            Refusal::TooMany => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, HeaderValue::from_static("1"))],
            )
                .into_response(),
            Refusal::Plain => StatusCode::FORBIDDEN.into_response(),
            Refusal::Unavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    /// `Basic realm="<brand name>", charset="UTF-8"` (RFC 7617), the name quoted.
    fn challenge(&self) -> HeaderValue {
        let realm = self.brand_name.replace('\\', "\\\\").replace('"', "\\\"");
        HeaderValue::from_bytes(format!("Basic realm=\"{realm}\", charset=\"UTF-8\"").as_bytes())
            .unwrap_or_else(|_| {
                HeaderValue::from_static("Basic realm=\"recto\", charset=\"UTF-8\"")
            })
    }
}

/// Who the request's device password belongs to (see [`secret`] for where it is read), or why
/// it is refused. Checked in this order: plain HTTP, missing credentials, the peer's failures,
/// then the database, on every request, so a revoked password or a disabled user is refused at
/// the next request.
pub async fn authenticate(
    state: &DeviceAuth,
    parts: &Parts,
    path_secret: Option<&str>,
) -> Result<DeviceUser, Refusal> {
    let peer = peer(parts);
    if !state.plain_allowed && !peer.is_loopback() {
        tracing::warn!(%peer, "device credentials refused over plain HTTP");
        return Err(Refusal::Plain);
    }
    let (secret, in_path): (Cow<'_, str>, bool) = match secret(parts, path_secret) {
        Secret::Missing => return Err(Refusal::Missing),
        Secret::Basic(password) => (Cow::Owned(password), false),
        Secret::Path(secret) => (Cow::Borrowed(secret), true),
    };
    if !state.limiter.allow(peer) {
        tracing::warn!(%peer, "device password refused: too many failures");
        return Err(Refusal::TooMany);
    }
    match device_passwords::authenticate(&state.database, &secret, chrono::Utc::now()).await {
        Ok(Some(user)) => Ok(user),
        Ok(None) => {
            state.limiter.failed(peer);
            tracing::warn!(%peer, in_path, "unknown device password");
            Err(Refusal::Wrong { in_path })
        }
        Err(error) => {
            tracing::error!(%error, "checking a device password failed");
            Err(Refusal::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::Request;
    use recto_domain::{
        passwords::PasswordPolicy,
        users::{self, NewLocalUser},
    };
    use url::Url;

    use super::*;

    /// Device authentication over a fresh database with one user, alice, and the secret of
    /// her device password.
    async fn setup(plain_allowed: bool) -> (DeviceAuth, String) {
        let database = recto_domain::testing::database().await;
        let alice = users::create_local(
            &database,
            NewLocalUser {
                username: "alice".into(),
                display_name: "Alice".into(),
                email: None,
                password: "correct horse battery staple".into(),
                is_administrator: false,
            },
            &PasswordPolicy { minimum_length: 12 },
        )
        .await
        .unwrap();
        let created = device_passwords::create(&database, alice.id, "Phone", chrono::Utc::now())
            .await
            .unwrap();
        let auth = DeviceAuth {
            database,
            limiter: DeviceLimiter::new(),
            plain_allowed,
            brand_name: Arc::from("recto"),
        };
        (auth, created.secret)
    }

    fn request(peer: IpAddr, authorization: Option<&str>) -> Parts {
        let mut request = Request::post("/ipp/print");
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        let mut parts = request.body(()).unwrap().into_parts().0;
        parts
            .extensions
            .insert(ConnectInfo(SocketAddr::new(peer, 50_000)));
        parts
    }

    fn basic(username: &str, password: &str) -> String {
        format!(
            "Basic {}",
            BASE64.encode(format!("{username}:{password}").as_bytes())
        )
    }

    const LAN: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5));

    #[tokio::test]
    async fn basic_and_path_secrets_authenticate() {
        let (auth, secret) = setup(true).await;
        // The username is whatever the computer fills in.
        let user = authenticate(&auth, &request(LAN, Some(&basic("me", &secret))), None)
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.device_name, "Phone");
        let user = authenticate(&auth, &request(LAN, None), Some(&secret))
            .await
            .unwrap();
        assert_eq!(user.username, "alice");

        let missing = authenticate(&auth, &request(LAN, None), None).await;
        assert_eq!(missing.unwrap_err(), Refusal::Missing);
        let no_colon = format!("Basic {}", BASE64.encode(b"no colon"));
        for unreadable in ["Basic !!!", "Bearer token", "Basic", &no_colon] {
            let refusal = authenticate(&auth, &request(LAN, Some(unreadable)), None).await;
            assert_eq!(refusal.unwrap_err(), Refusal::Missing, "{unreadable}");
        }
        // The account password is not a device password.
        let account = basic("alice", "correct horse battery staple");
        let wrong = authenticate(&auth, &request(LAN, Some(&account)), None)
            .await
            .unwrap_err();
        assert_eq!(wrong, Refusal::Wrong { in_path: false });
        let response = auth.refuse(wrong);
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::WWW_AUTHENTICATE],
            "Basic realm=\"recto\", charset=\"UTF-8\""
        );
        let response = auth.refuse(Refusal::Missing);
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));

        let wrong = authenticate(&auth, &request(LAN, None), Some(&"a".repeat(32)))
            .await
            .unwrap_err();
        assert_eq!(wrong, Refusal::Wrong { in_path: true });
        let response = auth.refuse(wrong);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
        assert!(!response.headers().contains_key(header::LOCATION));
    }

    #[tokio::test]
    async fn the_realm_is_the_quoted_brand_name() {
        let (mut auth, _) = setup(true).await;
        auth.brand_name = Arc::from("Chez \"Nous\" \\ Imprimerie");
        assert_eq!(
            auth.refuse(Refusal::Missing).headers()[header::WWW_AUTHENTICATE].as_bytes(),
            "Basic realm=\"Chez \\\"Nous\\\" \\\\ Imprimerie\", charset=\"UTF-8\"".as_bytes()
        );
    }

    #[tokio::test]
    async fn polling_is_never_limited_but_floods_are() {
        let (auth, secret) = setup(true).await;
        let good = request(LAN, Some(&basic("alice", &secret)));
        for _ in 0..1000 {
            assert!(authenticate(&auth, &good, None).await.is_ok());
        }
        let wrong = request(LAN, Some(&basic("alice", &"b".repeat(32))));
        for _ in 0..30 {
            assert_eq!(
                authenticate(&auth, &wrong, None).await.unwrap_err(),
                Refusal::Wrong { in_path: false }
            );
        }
        // One failure comes back each second; a slow machine may have earned one or two.
        let mut refusal = Refusal::Wrong { in_path: false };
        for _ in 0..5 {
            refusal = authenticate(&auth, &wrong, None).await.unwrap_err();
            if refusal == Refusal::TooMany {
                break;
            }
        }
        assert_eq!(refusal, Refusal::TooMany);
        let response = auth.refuse(refusal);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        // Over budget, the address is refused before any lookup, even with the right secret.
        assert_eq!(
            authenticate(&auth, &good, None).await.unwrap_err(),
            Refusal::TooMany
        );
        // Other addresses are not affected.
        let other = request(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 6)),
            Some(&basic("alice", &secret)),
        );
        assert!(authenticate(&auth, &other, None).await.is_ok());
    }

    #[test]
    fn failures_come_back_at_one_per_second() {
        let limiter = DeviceLimiter::new();
        let start = Instant::now();
        for _ in 0..30 {
            assert!(limiter.allow_at(LAN, start));
            limiter.failed_at(LAN, start);
        }
        assert!(!limiter.allow_at(LAN, start));
        assert!(!limiter.allow_at(LAN, start + Duration::from_millis(900)));
        assert!(limiter.allow_at(LAN, start + Duration::from_secs(1)));
        limiter.failed_at(LAN, start + Duration::from_secs(1));
        assert!(!limiter.allow_at(LAN, start + Duration::from_secs(1)));
        // A client of the same IPv6 /64 network shares the budget.
        let first: IpAddr = "2001:db8::1".parse().unwrap();
        let second: IpAddr = "2001:db8::2".parse().unwrap();
        for _ in 0..30 {
            limiter.failed_at(first, start);
        }
        assert!(!limiter.allow_at(second, start));
        assert!(limiter.allow_at("2001:db8:0:1::1".parse().unwrap(), start));
    }

    #[test]
    fn logged_paths_hide_personal_secrets() {
        let secret = "a".repeat(32);
        for (path, logged) in [
            (format!("/device/{secret}"), "/device/<secret>"),
            (format!("/device/{secret}/"), "/device/<secret>/"),
            (
                format!("/device/{secret}/ipp/print"),
                "/device/<secret>/ipp/print",
            ),
            ("/device/".to_owned(), "/device/"),
            ("/account/devices".to_owned(), "/account/devices"),
            ("/devices/abc".to_owned(), "/devices/abc"),
            ("/".to_owned(), "/"),
        ] {
            assert_eq!(loggable_path(&path), logged, "{path}");
        }
    }

    #[test]
    fn the_peer_is_the_tcp_peer() {
        let mut parts = request(LAN, None);
        parts
            .headers
            .insert("x-forwarded-for", HeaderValue::from_static("10.0.0.9"));
        parts
            .headers
            .insert("forwarded", HeaderValue::from_static("for=10.0.0.9"));
        assert_eq!(peer(&parts), LAN);
        let mapped = request("::ffff:192.168.1.5".parse().unwrap(), None);
        assert_eq!(peer(&mapped), LAN);
        let v6: IpAddr = "2001:db8::7".parse().unwrap();
        assert_eq!(peer(&request(v6, None)), v6);
    }

    #[tokio::test]
    async fn plain_http_is_refused_unless_allowed() {
        let (auth, secret) = setup(false).await;
        let credentials = basic("alice", &secret);
        let refusal = authenticate(&auth, &request(LAN, Some(&credentials)), None)
            .await
            .unwrap_err();
        assert_eq!(refusal, Refusal::Plain);
        assert_eq!(auth.refuse(refusal).status(), StatusCode::FORBIDDEN);
        // Even without credentials, so the device is not asked to send them in the clear.
        let refusal = authenticate(&auth, &request(LAN, None), None).await;
        assert_eq!(refusal.unwrap_err(), Refusal::Plain);
        for loopback in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            let request = request(loopback.parse().unwrap(), Some(&credentials));
            assert!(
                authenticate(&auth, &request, None).await.is_ok(),
                "{loopback}"
            );
        }
    }

    #[tokio::test]
    async fn plain_http_follows_the_public_url_and_the_setting() {
        let database = recto_domain::testing::database().await;
        let auth = |url: &str, allow: bool| AuthState {
            ipp_allow_plain: allow,
            ..AuthState::new(None, Url::parse(url).unwrap(), database.clone())
        };
        let plain = |url: &str, allow: bool| DeviceAuth::new(&auth(url, allow), "recto");
        assert!(plain("https://recto.example.org", false).plain_allowed);
        assert!(!plain("http://192.168.1.2:8080", false).plain_allowed);
        assert!(plain("http://192.168.1.2:8080", true).plain_allowed);
    }
}
