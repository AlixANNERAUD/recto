//! OpenID Connect sign-in against a mocked provider: discovery, PKCE, nonce, signed ID tokens,
//! required claims and the provider being down at first.

use std::time::Instant;

use chrono::{Duration, Utc};
use openidconnect::{
    AccessToken, Audience, EndUserEmail, EndUserName, EndUserUsername, IdToken, IdTokenClaims,
    IssuerUrl, JsonWebKeyId, LocalizedClaim, Nonce, PrivateSigningKey, StandardClaims,
    SubjectIdentifier,
    core::{
        CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
        CoreRsaPrivateSigningKey,
    },
};
use recto_authentication::{
    OpenId, OpenIdError, OpenIdSettings, PendingSignIn, SignedIn, openid::ExtraClaims,
};
use recto_domain::users;
use serde_json::{Value, json};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

const CLIENT_ID: &str = "recto";
// A key made for these tests only; it signs nothing real.
const SIGNING_KEY: &str = include_str!("fixtures/test-only-oidc-signing-key.pem");
const OTHER_KEY: &str = include_str!("fixtures/test-only-oidc-other-key.pem");

#[derive(Clone, Copy)]
enum Key {
    Signing,
    Other,
}

fn key(which: Key) -> CoreRsaPrivateSigningKey {
    let pem = match which {
        Key::Signing => SIGNING_KEY,
        Key::Other => OTHER_KEY,
    };
    // Both keys share an id, so a rotation is seen as a signature that no longer verifies
    // or a key that is gone, whichever the verifier meets first.
    CoreRsaPrivateSigningKey::from_pem(pem, Some(JsonWebKeyId::new("test-key".to_owned())))
        .expect("a valid test key")
}

const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// A mocked provider whose issuer is the bare origin, as Zitadel announces it.
struct Provider {
    server: MockServer,
    /// Announce the issuer with a trailing slash, as Authentik does.
    slash: bool,
    /// Advertise an `end_session_endpoint`.
    end_session: bool,
    /// Advertise a `userinfo_endpoint`.
    user_info: bool,
}

impl Provider {
    /// A provider that answers discovery.
    async fn start() -> Self {
        let provider = Self::down().await;
        provider.mount_discovery().await;
        provider
    }

    async fn start_with(slash: bool, end_session: bool) -> Self {
        let provider = Self {
            server: MockServer::start().await,
            slash,
            end_session,
            user_info: true,
        };
        provider.mount_discovery().await;
        provider
    }

    /// A provider that answers nothing yet, as when the identity provider is down.
    async fn down() -> Self {
        Self {
            server: MockServer::start().await,
            slash: false,
            end_session: true,
            user_info: true,
        }
    }

    /// A provider without a UserInfo endpoint, whose ID tokens carry every claim.
    async fn without_user_info() -> Self {
        let provider = Self {
            user_info: false,
            ..Self::down().await
        };
        provider.mount_discovery().await;
        provider
    }

    /// The issuer as the provider announces it, in discovery and in ID tokens.
    fn issuer(&self) -> String {
        if self.slash {
            format!("{}/", self.server.uri())
        } else {
            self.server.uri()
        }
    }

    fn settings(&self) -> OpenIdSettings {
        OpenIdSettings {
            // `Url` always writes the trailing slash a bare origin gets.
            issuer: Url::parse(&self.server.uri()).unwrap(),
            client_id: CLIENT_ID.to_owned(),
            redirect_url: "https://recto.example.org/auth/callback".parse().unwrap(),
            extra_scopes: Vec::new(),
            required_claim: None,
            required_value: None,
            administrator_claim: None,
            administrator_value: None,
            groups_claim: None,
        }
    }

    async fn mount_discovery(&self) {
        self.mount_discovery_with(Key::Signing).await;
    }

    /// Serves discovery, and the key set with the public part of `published`.
    async fn mount_discovery_with(&self, published: Key) {
        let base = self.server.uri();
        let mut document = json!({
            "issuer": self.issuer(),
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "jwks_uri": format!("{base}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        });
        if self.end_session {
            document["end_session_endpoint"] = json!(format!("{base}/logout"));
        }
        if self.user_info {
            document["userinfo_endpoint"] = json!(format!("{base}/userinfo"));
        }
        Mock::given(method("GET"))
            .and(path(DISCOVERY_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(document))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"keys": [key(published).as_verification_key()]})),
            )
            .mount(&self.server)
            .await;
    }

    async fn user_information_requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == "/userinfo")
            .count()
    }

    async fn discovery_requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path() == DISCOVERY_PATH)
            .count()
    }

    /// Answers the next code exchange as described by `answer`.
    async fn respond(&self, answer: Answer) {
        self.server.reset().await;
        self.mount_discovery_with(answer.published).await;
        let access_token = AccessToken::new("access-token".to_owned());
        let claims = serde_json::from_value(answer.extra).unwrap();
        let mut standard = StandardClaims::new(SubjectIdentifier::new(answer.subject.clone()));
        if answer.profile {
            standard = standard
                .set_preferred_username(Some(EndUserUsername::new("alice".to_owned())))
                .set_name(Some(LocalizedClaim::from(EndUserName::new(
                    "Alice Martin".to_owned(),
                ))))
                .set_email(Some(EndUserEmail::new("alice@example.org".to_owned())));
        }
        let id_token = IdToken::<
            ExtraClaims,
            CoreGenderClaim,
            CoreJweContentEncryptionAlgorithm,
            CoreJwsSigningAlgorithm,
        >::new(
            IdTokenClaims::new(
                IssuerUrl::new(self.issuer()).unwrap(),
                vec![Audience::new(answer.audience)],
                Utc::now() + answer.lifetime,
                Utc::now(),
                standard,
                ExtraClaims { claims },
            )
            .set_nonce(Some(Nonce::new(answer.nonce))),
            &key(answer.signed_with),
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            Some(&access_token),
            None,
        )
        .unwrap();
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code_verifier="))
            .and(body_string_contains("code=the-code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "access-token",
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": id_token.to_string()
            })))
            .mount(&self.server)
            .await;
        let mut information = answer.information;
        if information.get("sub").is_none() {
            information["sub"] = json!(answer.subject);
        }
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(
                ResponseTemplate::new(answer.information_status).set_body_json(information),
            )
            .mount(&self.server)
            .await;
    }
}

/// What the provider answers at the token and user information endpoints; the defaults are a
/// valid sign-in.
struct Answer {
    nonce: String,
    subject: String,
    audience: String,
    lifetime: Duration,
    signed_with: Key,
    published: Key,
    extra: Value,
    information: Value,
    /// The status of the user information answer.
    information_status: u16,
    /// Put `preferred_username`, `name` and `email` in the ID token, as Zitadel does only when
    /// "User Info inside ID Token" is on.
    profile: bool,
}

impl Answer {
    fn new(nonce: &str) -> Self {
        Self {
            nonce: nonce.to_owned(),
            subject: "subject-1".to_owned(),
            audience: CLIENT_ID.to_owned(),
            lifetime: Duration::minutes(5),
            signed_with: Key::Signing,
            published: Key::Signing,
            extra: json!({}),
            information: json!({}),
            information_status: 200,
            profile: true,
        }
    }
}

fn query_parameter(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| panic!("{name} is missing from {url}"))
}

async fn sign_in(
    provider: &Provider,
    openid: &OpenId,
    extra: Value,
    information: Value,
) -> Result<SignedIn, OpenIdError> {
    sign_in_with(provider, openid, |answer| {
        answer.extra = extra;
        answer.information = information;
    })
    .await
}

async fn sign_in_with(
    provider: &Provider,
    openid: &OpenId,
    customize: impl FnOnce(&mut Answer),
) -> Result<SignedIn, OpenIdError> {
    let (url, pending) = openid.start().await.unwrap();
    let state = query_parameter(&url, "state");
    let mut answer = Answer::new(&query_parameter(&url, "nonce"));
    customize(&mut answer);
    provider.respond(answer).await;
    openid.finish(pending, "the-code", &state).await
}

#[tokio::test]
async fn full_flow_returns_user() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    assert_eq!(signed_in.identity.subject, "subject-1");
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice")
    );
    assert_eq!(signed_in.identity.name.as_deref(), Some("Alice Martin"));
    assert!(!signed_in.id_token.is_empty());

    let post_logout: Url = "https://recto.example.org/".parse().unwrap();
    let logout = openid
        .end_session_url(Some(&signed_in.id_token), &post_logout)
        .expect("the provider announces an end session endpoint");
    assert_eq!(logout.path(), "/logout");
    assert_eq!(
        query_parameter(&logout, "id_token_hint"),
        signed_in.id_token
    );
    assert_eq!(
        query_parameter(&logout, "post_logout_redirect_uri"),
        post_logout.as_str()
    );
}

#[tokio::test]
async fn authorization_url_has_pkce_and_scopes() {
    let provider = Provider::start().await;
    let openid = OpenId::new(OpenIdSettings {
        extra_scopes: vec!["groups".to_owned()],
        ..provider.settings()
    });
    let (url, pending) = openid.start().await.unwrap();
    assert_eq!(url.path(), "/authorize");
    assert_eq!(query_parameter(&url, "code_challenge_method"), "S256");
    assert!(!query_parameter(&url, "code_challenge").is_empty());
    assert_eq!(query_parameter(&url, "client_id"), CLIENT_ID);
    assert_eq!(
        query_parameter(&url, "scope"),
        "openid profile email groups"
    );
    assert_eq!(query_parameter(&url, "state"), pending.state);
    assert_eq!(query_parameter(&url, "nonce"), pending.nonce);

    let stored = serde_json::to_string(&pending).unwrap();
    let restored: PendingSignIn = serde_json::from_str(&stored).unwrap();
    assert_eq!(restored, pending);
}

#[tokio::test]
async fn state_mismatch_is_rejected() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let (_, pending) = openid.start().await.unwrap();
    let result = openid.finish(pending, "the-code", "another-state").await;
    assert!(matches!(result, Err(OpenIdError::StateMismatch)));
}

#[tokio::test]
async fn required_claim_missing_denies() {
    let provider = Provider::start().await;
    let openid = OpenId::new(OpenIdSettings {
        required_claim: Some("urn:zitadel:iam:org:project:roles".to_owned()),
        required_value: Some("recto".to_owned()),
        ..provider.settings()
    });
    let result = sign_in(&provider, &openid, json!({}), json!({})).await;
    assert!(matches!(result, Err(OpenIdError::AccessDenied)));

    let wrong = json!({"urn:zitadel:iam:org:project:roles": {"other": {"1": "example.org"}}});
    let result = sign_in(&provider, &openid, wrong, json!({})).await;
    assert!(matches!(result, Err(OpenIdError::AccessDenied)));

    let right = json!({"urn:zitadel:iam:org:project:roles": {"recto": {"1": "example.org"}}});
    assert!(sign_in(&provider, &openid, right, json!({})).await.is_ok());
}

#[tokio::test]
async fn required_claim_from_userinfo() {
    let provider = Provider::start().await;
    let openid = OpenId::new(OpenIdSettings {
        required_claim: Some("groups".to_owned()),
        required_value: Some("printers".to_owned()),
        ..provider.settings()
    });
    let signed_in = sign_in(
        &provider,
        &openid,
        json!({}),
        json!({"groups": ["staff", "printers"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice")
    );
}

#[tokio::test]
async fn required_claim_with_failing_userinfo_is_unavailable() {
    // Whether the person may sign in is unknown: a retry later, not a refusal.
    let provider = Provider::start().await;
    let openid = OpenId::new(OpenIdSettings {
        required_claim: Some("groups".to_owned()),
        required_value: Some("printers".to_owned()),
        ..provider.settings()
    });
    let result = sign_in_with(&provider, &openid, |answer| answer.information_status = 500).await;
    assert!(
        matches!(result, Err(OpenIdError::Unavailable(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn provider_down_then_up() {
    let provider = Provider::down().await;
    let openid = OpenId::new(provider.settings()).retry_after(std::time::Duration::ZERO);
    match openid.start().await {
        Err(OpenIdError::Unavailable(reason)) => assert!(!reason.is_empty()),
        other => panic!("expected Unavailable, got {other:?}"),
    }
    provider.mount_discovery().await;
    assert!(openid.start().await.is_ok());
}

#[tokio::test]
async fn a_failed_discovery_is_not_retried_at_once() {
    let provider = Provider::down().await;
    let openid = OpenId::new(provider.settings());
    assert!(matches!(
        openid.start().await,
        Err(OpenIdError::Unavailable(_))
    ));
    // The provider is back, but sign-in pages keep failing fast for a few seconds.
    provider.mount_discovery().await;
    assert!(matches!(
        openid.start().await,
        Err(OpenIdError::Unavailable(_))
    ));
}

#[tokio::test]
async fn a_hanging_provider_fails_within_the_timeout_and_then_fast() {
    let provider = Provider::down().await;
    Mock::given(method("GET"))
        .and(path(DISCOVERY_PATH))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(30)))
        .mount(&provider.server)
        .await;
    let openid = OpenId::new(provider.settings());

    let started = Instant::now();
    assert!(matches!(
        openid.start().await,
        Err(OpenIdError::Unavailable(_))
    ));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(11),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(provider.discovery_requests().await, 1, "one spelling only");

    let started = Instant::now();
    assert!(matches!(
        openid.start().await,
        Err(OpenIdError::Unavailable(_))
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn rotated_signing_key_is_picked_up() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    // Discovery happens at the first start and caches key A.
    let (url, pending) = openid.start().await.unwrap();
    let state = query_parameter(&url, "state");
    let mut answer = Answer::new(&query_parameter(&url, "nonce"));
    answer.signed_with = Key::Other;
    answer.published = Key::Other;
    provider.respond(answer).await;
    let signed_in = openid.finish(pending, "the-code", &state).await.unwrap();
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice")
    );
    assert_eq!(provider.discovery_requests().await, 1, "one re-discovery");

    // The new keys are kept: the next sign-in needs no discovery.
    sign_in_with(&provider, &openid, |answer| {
        answer.signed_with = Key::Other;
        answer.published = Key::Other;
    })
    .await
    .unwrap();
    assert_eq!(provider.discovery_requests().await, 0);
}

#[tokio::test]
async fn token_signed_by_an_unknown_key_is_refused_after_one_re_discovery() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let result = sign_in_with(&provider, &openid, |answer| {
        answer.signed_with = Key::Other;
    })
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
    assert_eq!(provider.discovery_requests().await, 1);
}

#[tokio::test]
async fn wrong_nonce_is_refused() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let result = sign_in_with(&provider, &openid, |answer| {
        answer.nonce = "someone-elses-nonce".to_owned();
    })
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn wrong_audience_is_refused() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let result = sign_in_with(&provider, &openid, |answer| {
        answer.audience = "another-client".to_owned();
    })
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn expired_token_is_refused() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let result = sign_in_with(&provider, &openid, |answer| {
        answer.lifetime = Duration::minutes(-5);
    })
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn user_information_for_another_subject_grants_nothing() {
    let provider = Provider::start().await;
    let openid = OpenId::new(OpenIdSettings {
        required_claim: Some("groups".to_owned()),
        required_value: Some("printers".to_owned()),
        ..provider.settings()
    });
    let result = sign_in(
        &provider,
        &openid,
        json!({}),
        json!({"sub": "someone-else", "groups": ["printers"]}),
    )
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn issuer_announced_with_a_trailing_slash_works() {
    let provider = Provider::start_with(true, true).await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice")
    );
}

#[tokio::test]
async fn no_end_session_endpoint_means_no_sign_out_url() {
    let provider = Provider::start_with(false, false).await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    let post_logout: Url = "https://recto.example.org/".parse().unwrap();
    assert!(
        openid
            .end_session_url(Some(&signed_in.id_token), &post_logout)
            .is_none()
    );
}

#[tokio::test]
async fn debug_output_hides_secrets() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let (_, pending) = openid.start().await.unwrap();
    let shown = format!("{pending:?}");
    for secret in [&pending.state, &pending.nonce, &pending.pkce_verifier] {
        assert!(!shown.contains(secret.as_str()), "{shown}");
    }
    let signed_in = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    assert!(!format!("{signed_in:?}").contains(&signed_in.id_token));
}

#[tokio::test]
async fn username_comes_from_user_information_when_the_id_token_has_no_profile() {
    // Zitadel leaves the profile out of the ID token unless told otherwise; without it the
    // username would be the numeric subject.
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in_with(&provider, &openid, |answer| {
        answer.profile = false;
        answer.information = json!({
            "preferred_username": "alice.martin",
            "name": "Alice Martin",
        });
    })
    .await
    .unwrap();
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice.martin")
    );
    assert_eq!(signed_in.identity.name.as_deref(), Some("Alice Martin"));
    assert_eq!(provider.user_information_requests().await, 1);
}

#[tokio::test]
async fn the_id_token_profile_needs_no_user_information() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in_with(&provider, &openid, |answer| {
        answer.information = json!({"preferred_username": "someone-else"});
    })
    .await
    .unwrap();
    assert_eq!(
        signed_in.identity.preferred_username.as_deref(),
        Some("alice")
    );
    assert_eq!(provider.user_information_requests().await, 0);
}

#[tokio::test]
async fn a_profile_for_another_subject_is_refused() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let result = sign_in_with(&provider, &openid, |answer| {
        answer.profile = false;
        answer.information = json!({"sub": "someone-else", "preferred_username": "mallory"});
    })
    .await;
    assert!(
        matches!(result, Err(OpenIdError::Rejected(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn sign_out_without_an_id_token_names_the_client() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    openid.start().await.unwrap();
    let post_logout: Url = "https://recto.example.org/auth/signed-out".parse().unwrap();
    let logout = openid
        .end_session_url(None, &post_logout)
        .expect("the provider announces an end session endpoint");
    assert_eq!(query_parameter(&logout, "client_id"), CLIENT_ID);
    assert_eq!(
        query_parameter(&logout, "post_logout_redirect_uri"),
        post_logout.as_str()
    );
    assert!(
        !logout
            .query_pairs()
            .any(|(name, _)| name == "id_token_hint")
    );
}

fn administrators(provider: &Provider) -> OpenId {
    OpenId::new(OpenIdSettings {
        administrator_claim: Some("roles".to_owned()),
        administrator_value: Some("admin".to_owned()),
        groups_claim: Some("groups".to_owned()),
        ..provider.settings()
    })
}

#[tokio::test]
async fn identity_carries_issuer_subject_and_profile() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let identity = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap()
        .identity;
    assert_eq!(identity.issuer, provider.settings().issuer.as_str());
    assert_eq!(identity.subject, "subject-1");
    assert_eq!(identity.preferred_username.as_deref(), Some("alice"));
    assert_eq!(identity.email.as_deref(), Some("alice@example.org"));
    assert_eq!(identity.name.as_deref(), Some("Alice Martin"));
    assert_eq!(identity.administrator, None);
    assert_eq!(identity.groups, None);
}

#[tokio::test]
async fn administrator_claim_is_read() {
    let provider = Provider::start().await;
    let openid = administrators(&provider);
    let admin = sign_in(&provider, &openid, json!({"roles": ["admin"]}), json!({}))
        .await
        .unwrap();
    assert_eq!(admin.identity.administrator, Some(true));
    let other = sign_in(&provider, &openid, json!({"roles": ["user"]}), json!({}))
        .await
        .unwrap();
    assert_eq!(other.identity.administrator, Some(false));

    let unconfigured = OpenId::new(provider.settings());
    let none = sign_in(
        &provider,
        &unconfigured,
        json!({"roles": ["admin"]}),
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(none.identity.administrator, None);
}

#[tokio::test]
async fn groups_claim_is_read_from_strings_lists_and_objects() {
    let provider = Provider::start().await;
    let openid = administrators(&provider);
    for (claim, expected) in [
        (json!("staff"), vec!["staff"]),
        (json!(["staff", "printers"]), vec!["staff", "printers"]),
        (json!({"staff": {"1": "x"}}), vec!["staff"]),
    ] {
        let groups = sign_in(&provider, &openid, json!({"groups": claim}), json!({}))
            .await
            .unwrap()
            .identity
            .groups;
        assert_eq!(
            groups,
            Some(expected.into_iter().map(String::from).collect())
        );
    }
}

#[tokio::test]
async fn missing_admin_or_group_claims_are_fetched_from_userinfo() {
    let provider = Provider::start().await;
    let openid = administrators(&provider);
    let identity = sign_in(
        &provider,
        &openid,
        json!({}),
        json!({"roles": ["admin"], "groups": ["staff"]}),
    )
    .await
    .unwrap()
    .identity;
    assert_eq!(identity.administrator, Some(true));
    assert_eq!(identity.groups, Some(vec!["staff".to_owned()]));
    assert_eq!(provider.user_information_requests().await, 1);

    // UserInfo answered and has neither claim: the user has no groups and is no administrator.
    let identity = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap()
        .identity;
    assert_eq!(identity.administrator, Some(false));
    assert_eq!(identity.groups, Some(Vec::new()));
}

#[tokio::test]
async fn userinfo_failure_leaves_admin_and_groups_unknown() {
    let provider = Provider::start().await;
    let openid = administrators(&provider);
    let identity = sign_in_with(&provider, &openid, |answer| answer.information_status = 500)
        .await
        .unwrap()
        .identity;
    assert_eq!(identity.administrator, None);
    assert_eq!(identity.groups, None);

    // What the ID token does carry still counts.
    let identity = sign_in_with(&provider, &openid, |answer| {
        answer.information_status = 500;
        answer.extra = json!({"roles": ["admin"]});
    })
    .await
    .unwrap()
    .identity;
    assert_eq!(identity.administrator, Some(true));
    assert_eq!(identity.groups, None);
}

#[tokio::test]
async fn without_userinfo_claims_missing_from_the_id_token_are_absent() {
    let provider = Provider::without_user_info().await;
    let openid = administrators(&provider);
    let database = recto_domain::testing::database().await;
    let granted = sign_in(
        &provider,
        &openid,
        json!({"roles": ["admin"], "groups": ["staff"]}),
        json!({}),
    )
    .await
    .unwrap();
    let user = users::sign_in_with_identity(&database, &granted.identity)
        .await
        .unwrap();
    assert!(user.is_administrator && user.administrator_from_claim);

    // Nothing else could carry the claims, so their absence takes back what they granted.
    let revoked = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    assert_eq!(revoked.identity.administrator, Some(false));
    assert_eq!(revoked.identity.groups, Some(Vec::new()));
    assert_eq!(provider.user_information_requests().await, 0);
    let user = users::sign_in_with_identity(&database, &revoked.identity)
        .await
        .unwrap();
    assert!(!user.is_administrator);
    assert!(!user.administrator_from_claim);
}
