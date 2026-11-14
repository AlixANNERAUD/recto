//! Sign-in with any OpenID Connect provider: authorization code flow with PKCE and a nonce,
//! as a public client (no client secret).
//!
//! Security choices worth knowing before changing this module:
//!
//! - the HTTP client does not follow redirects, as the `openidconnect` crate recommends: a
//!   redirect from the provider's endpoints could otherwise send server-side requests to
//!   internal addresses;
//! - the provider is discovered on first use rather than at startup, so recto starts even when
//!   the provider is down; a failed discovery is retried by the next call;
//! - claims come from the ID token, and from UserInfo only when the required claim is missing
//!   from the token, since providers differ on where they put roles.

use std::time::Duration;

use openidconnect::{
    AccessTokenHash, AdditionalClaims, AuthorizationCode, Client, ClientId, CsrfToken,
    EmptyAdditionalProviderMetadata, EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IdTokenFields, IssuerUrl, LogoutProviderMetadata, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, ProviderMetadataWithLogout, RedirectUrl,
    RequestTokenError, Scope, StandardErrorResponse, StandardTokenResponse, TokenResponse,
    UserInfoClaims,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
        CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm, CoreRevocableToken, CoreRevocationErrorResponse,
        CoreTokenIntrospectionResponse, CoreTokenType,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::OnceCell;
use url::Url;

use crate::{claims, user::User};

/// Every claim of the ID token and UserInfo besides the standard ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtraClaims {
    #[serde(flatten)]
    pub claims: std::collections::HashMap<String, Value>,
}

impl AdditionalClaims for ExtraClaims {}

type ExtraIdTokenFields = IdTokenFields<
    ExtraClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;
type ExtraTokenResponse = StandardTokenResponse<ExtraIdTokenFields, CoreTokenType>;
type DiscoveredClient = Client<
    ExtraClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    ExtraTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// OpenID Connect settings, from the environment.
#[derive(Debug, Clone)]
pub struct OpenIdSettings {
    /// The issuer URL; discovery reads `{issuer}/.well-known/openid-configuration`.
    pub issuer: Url,
    pub client_id: String,
    /// The callback address registered with the provider.
    pub redirect_url: Url,
    /// Scopes requested besides `openid profile email`.
    pub extra_scopes: Vec<String>,
    /// A claim the user must have to be let in (see [`claims::allows`]); anyone the provider
    /// authenticates is let in when unset.
    pub required_claim: Option<String>,
    /// The value that claim must hold; its presence is enough when unset.
    pub required_value: Option<String>,
}

/// What the callback needs from the start of a sign-in, kept in the session meanwhile. It
/// must be used once and then removed from the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSignIn {
    /// Ties the callback to the browser that started the sign-in.
    pub state: String,
    /// Expected in the ID token, so a token issued for another sign-in is refused.
    pub nonce: String,
    pub pkce_verifier: String,
}

/// A completed sign-in.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub user: User,
    /// Kept in the session to hint the provider at sign-out.
    pub id_token: String,
}

/// Why a sign-in failed. The messages reach the sign-in error page, so they stay readable.
#[derive(Debug, thiserror::Error)]
pub enum OpenIdError {
    #[error("sign-in provider unavailable: {0}")]
    Unavailable(String),
    #[error("the sign-in request does not match the one started here; try again")]
    StateMismatch,
    #[error("the sign-in provider refused the sign-in: {0}")]
    Rejected(String),
    #[error("your account does not have the access required to use recto")]
    AccessDenied,
}

/// The provider as discovered: the client and where to send the browser to sign out.
struct Discovered {
    client: DiscoveredClient,
    end_session_endpoint: Option<Url>,
}

/// The OpenID Connect provider, discovered on first use.
pub struct OpenId {
    settings: OpenIdSettings,
    http: reqwest::Client,
    discovered: OnceCell<Discovered>,
}

impl OpenId {
    /// Prepares the client without any network access.
    pub fn new(settings: OpenIdSettings) -> Self {
        let http = reqwest::Client::builder()
            // Following redirects would expose the client to server-side request forgery.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            // Only fails when the TLS backend cannot initialize, which no retry can fix.
            .expect("the HTTP client could not be built");
        Self {
            settings,
            http,
            discovered: OnceCell::new(),
        }
    }

    /// Discovers the provider, once; a failed discovery is retried by the next call.
    async fn discovered(&self) -> Result<&Discovered, OpenIdError> {
        self.discovered
            .get_or_try_init(|| async {
                let unavailable =
                    |error: &dyn std::fmt::Display| OpenIdError::Unavailable(error.to_string());
                let mut first_error = None;
                let mut found = None;
                for issuer in issuer_candidates(&self.settings.issuer) {
                    let issuer = IssuerUrl::new(issuer).map_err(|error| unavailable(&error))?;
                    match ProviderMetadataWithLogout::discover_async(issuer, &self.http).await {
                        Ok(metadata) => {
                            found = Some(metadata);
                            break;
                        }
                        Err(error) => {
                            first_error.get_or_insert_with(|| error.to_string());
                        }
                    }
                }
                let metadata = found
                    .ok_or_else(|| OpenIdError::Unavailable(first_error.unwrap_or_default()))?;
                let LogoutProviderMetadata {
                    end_session_endpoint,
                    additional_metadata: EmptyAdditionalProviderMetadata {},
                } = metadata.additional_metadata().clone();
                let redirect = RedirectUrl::from_url(self.settings.redirect_url.clone());
                let client = Client::from_provider_metadata(
                    metadata,
                    ClientId::new(self.settings.client_id.clone()),
                    None,
                )
                .set_redirect_uri(redirect);
                Ok(Discovered {
                    client,
                    end_session_endpoint: end_session_endpoint.map(|url| url.url().clone()),
                })
            })
            .await
    }

    /// Starts a sign-in: the provider address to redirect to, and what to keep in the session.
    pub async fn start(&self) -> Result<(Url, PendingSignIn), OpenIdError> {
        let discovered = self.discovered().await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = discovered.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for scope in ["profile", "email"] {
            request = request.add_scope(Scope::new(scope.to_owned()));
        }
        for scope in &self.settings.extra_scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        // The authorization code flow adds `openid` itself.
        let (url, state, nonce) = request.set_pkce_challenge(challenge).url();
        Ok((
            url,
            PendingSignIn {
                state: state.secret().clone(),
                nonce: nonce.secret().clone(),
                pkce_verifier: verifier.secret().clone(),
            },
        ))
    }

    /// Completes a sign-in from the provider's callback.
    ///
    /// Verifies the state, exchanges the code with the PKCE verifier, checks the ID token's
    /// signature, nonce and access token hash, then the required claim.
    pub async fn finish(
        &self,
        pending: PendingSignIn,
        code: &str,
        state: &str,
    ) -> Result<SignedIn, OpenIdError> {
        if state != pending.state {
            return Err(OpenIdError::StateMismatch);
        }
        let client = &self.discovered().await?.client;
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|error| OpenIdError::Rejected(error.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|error| match error {
                RequestTokenError::Request(error) => OpenIdError::Unavailable(error.to_string()),
                other => OpenIdError::Rejected(other.to_string()),
            })?;
        let id_token = token
            .id_token()
            .ok_or_else(|| OpenIdError::Rejected("no ID token in the answer".to_owned()))?;
        let verifier = client.id_token_verifier();
        let rejected = |error: &dyn std::fmt::Display| OpenIdError::Rejected(error.to_string());
        let id_claims = id_token
            .claims(&verifier, &Nonce::new(pending.nonce))
            .map_err(|error| rejected(&error))?;
        if let Some(expected) = id_claims.access_token_hash() {
            let algorithm = id_token.signing_alg().map_err(|error| rejected(&error))?;
            let key = id_token
                .signing_key(&verifier)
                .map_err(|error| rejected(&error))?;
            let actual = AccessTokenHash::from_token(token.access_token(), algorithm, key)
                .map_err(|error| rejected(&error))?;
            if actual != *expected {
                return Err(OpenIdError::Rejected(
                    "the access token does not match the ID token".to_owned(),
                ));
            }
        }

        let mut all = to_map(id_claims);
        if let Some(claim) = &self.settings.required_claim
            && !claims::allows(&all, claim, self.settings.required_value.as_deref())
        {
            // Some providers only put roles in UserInfo.
            let request = client
                .user_info(
                    token.access_token().clone(),
                    Some(id_claims.subject().clone()),
                )
                .map_err(|error| rejected(&error))?;
            match request.request_async(&self.http).await {
                Ok(information) => {
                    let information: UserInfoClaims<ExtraClaims, CoreGenderClaim> = information;
                    for (key, value) in to_map(&information) {
                        all.entry(key).or_insert(value);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "reading the user information failed");
                }
            }
            if !claims::allows(&all, claim, self.settings.required_value.as_deref()) {
                return Err(OpenIdError::AccessDenied);
            }
        }
        let user = User::from_claims(&all)
            .ok_or_else(|| OpenIdError::Rejected("the ID token has no subject".to_owned()))?;
        Ok(SignedIn {
            user,
            id_token: id_token.to_string(),
        })
    }

    /// Where to send the browser to end the session at the provider, when it supports it and
    /// has been discovered (a sign-in always discovers it first).
    pub fn end_session_url(&self, id_token: &str, post_logout: &Url) -> Option<Url> {
        let mut url = self.discovered.get()?.end_session_endpoint.clone()?;
        url.query_pairs_mut()
            .append_pair("id_token_hint", id_token)
            .append_pair("client_id", &self.settings.client_id)
            .append_pair("post_logout_redirect_uri", post_logout.as_str());
        Some(url)
    }
}

/// All claims, standard ones included, as one JSON object.
fn to_map(claims: &impl Serialize) -> Map<String, Value> {
    match serde_json::to_value(claims) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// The issuer strings to try. `Url` always writes `https://host/` for a bare origin, but most
/// providers announce `https://host` and the library compares the two character for
/// character, so a bare origin is tried without its slash first.
fn issuer_candidates(issuer: &Url) -> Vec<String> {
    let text = issuer.as_str();
    if issuer.path() == "/" && issuer.query().is_none() && issuer.fragment().is_none() {
        vec![text.trim_end_matches('/').to_owned(), text.to_owned()]
    } else {
        vec![text.to_owned()]
    }
}
