//! OpenID Connect sign-in against a mocked provider: discovery, PKCE, nonce, signed ID tokens,
//! required claims and the provider being down at first.

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
use serde_json::{Value, json};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

const CLIENT_ID: &str = "recto";
// A key made for these tests only; it signs nothing real.
const KEY: &str = include_str!("fixtures/test-only-oidc-signing-key.pem");

fn signing_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(KEY, Some(JsonWebKeyId::new("test-key".to_owned())))
        .expect("a valid test key")
}

/// A mocked provider whose issuer is the bare origin, as Zitadel announces it.
struct Provider {
    server: MockServer,
}

impl Provider {
    /// A provider that answers discovery.
    async fn start() -> Self {
        let provider = Self::down().await;
        provider.mount_discovery().await;
        provider
    }

    /// A provider that answers nothing yet, as when the identity provider is down.
    async fn down() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    fn issuer(&self) -> String {
        self.server.uri()
    }

    fn settings(&self) -> OpenIdSettings {
        OpenIdSettings {
            // `Url` always writes the trailing slash the provider does not announce.
            issuer: Url::parse(&self.issuer()).unwrap(),
            client_id: CLIENT_ID.to_owned(),
            redirect_url: "https://recto.example.org/auth/callback".parse().unwrap(),
            extra_scopes: Vec::new(),
            required_claim: None,
            required_value: None,
        }
    }

    async fn mount_discovery(&self) {
        let base = self.server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "userinfo_endpoint": format!("{base}/userinfo"),
                "end_session_endpoint": format!("{base}/logout"),
                "jwks_uri": format!("{base}/jwks"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"keys": [signing_key().as_verification_key()]})),
            )
            .mount(&self.server)
            .await;
    }

    /// Answers the next code exchange with an ID token for `subject` carrying `extra` claims,
    /// and the user information endpoint with `information`.
    async fn answer(&self, nonce: &str, subject: &str, extra: Value, information: Value) {
        self.server.reset().await;
        self.mount_discovery().await;
        let access_token = AccessToken::new("access-token".to_owned());
        let claims = serde_json::from_value(extra).unwrap();
        let id_token = IdToken::<
            ExtraClaims,
            CoreGenderClaim,
            CoreJweContentEncryptionAlgorithm,
            CoreJwsSigningAlgorithm,
        >::new(
            IdTokenClaims::new(
                IssuerUrl::new(self.issuer()).unwrap(),
                vec![Audience::new(CLIENT_ID.to_owned())],
                Utc::now() + Duration::minutes(5),
                Utc::now(),
                StandardClaims::new(SubjectIdentifier::new(subject.to_owned()))
                    .set_preferred_username(Some(EndUserUsername::new("alice".to_owned())))
                    .set_name(Some(LocalizedClaim::from(EndUserName::new(
                        "Alice Martin".to_owned(),
                    ))))
                    .set_email(Some(EndUserEmail::new("alice@example.org".to_owned()))),
                ExtraClaims { claims },
            )
            .set_nonce(Some(Nonce::new(nonce.to_owned()))),
            &signing_key(),
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
        let mut information = information;
        information["sub"] = json!(subject);
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(information))
            .mount(&self.server)
            .await;
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
    let (url, pending) = openid.start().await.unwrap();
    let state = query_parameter(&url, "state");
    let nonce = query_parameter(&url, "nonce");
    provider
        .answer(&nonce, "subject-1", extra, information)
        .await;
    openid.finish(pending, "the-code", &state).await
}

#[tokio::test]
async fn full_flow_returns_user() {
    let provider = Provider::start().await;
    let openid = OpenId::new(provider.settings());
    let signed_in = sign_in(&provider, &openid, json!({}), json!({}))
        .await
        .unwrap();
    assert_eq!(signed_in.user.subject, "subject-1");
    assert_eq!(signed_in.user.username, "alice");
    assert_eq!(signed_in.user.name.as_deref(), Some("Alice Martin"));
    assert!(!signed_in.id_token.is_empty());

    let post_logout: Url = "https://recto.example.org/".parse().unwrap();
    let logout = openid
        .end_session_url(&signed_in.id_token, &post_logout)
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
    assert_eq!(signed_in.user.username, "alice");
}

#[tokio::test]
async fn provider_down_then_up() {
    let provider = Provider::down().await;
    let openid = OpenId::new(provider.settings());
    match openid.start().await {
        Err(OpenIdError::Unavailable(reason)) => assert!(!reason.is_empty()),
        other => panic!("expected Unavailable, got {other:?}"),
    }
    provider.mount_discovery().await;
    assert!(openid.start().await.is_ok());
}
