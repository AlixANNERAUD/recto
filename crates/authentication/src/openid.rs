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
//! - claims come from the ID token, and from UserInfo only when the required claim or every
//!   profile claim (`preferred_username`, `email`, `name`) is missing from the token: providers
//!   differ on where they put roles, and Zitadel leaves the profile out of the ID token unless
//!   told otherwise. ID token claims win over UserInfo ones.

use std::{
    sync::{Arc, PoisonError, RwLock},
    time::{Duration, Instant},
};

use openidconnect::{
    AccessTokenHash, AdditionalClaims, AuthorizationCode, ClaimsVerificationError, Client,
    ClientId, CsrfToken, DiscoveryError, EmptyAdditionalProviderMetadata, EmptyExtraTokenFields,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, IdTokenFields, IssuerUrl,
    LogoutProviderMetadata, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    ProviderMetadataWithLogout, RedirectUrl, RequestTokenError, Scope, SignatureVerificationError,
    StandardErrorResponse, StandardTokenResponse, TokenResponse, UserInfoClaims, UserInfoError,
    core::{
        CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
        CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm, CoreRevocableToken, CoreRevocationErrorResponse,
        CoreTokenIntrospectionResponse, CoreTokenType,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use url::Url;

use recto_domain::users::Identity;

use crate::claims;

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
    /// A claim that makes the user an administrator, matched like `required_claim`. Unset
    /// leaves administration to the database.
    pub administrator_claim: Option<String>,
    /// The value that claim must hold; its presence is enough when unset.
    pub administrator_value: Option<String>,
    /// A claim listing the user's groups (a string, a list of strings or an object whose keys
    /// are the groups). Unset leaves group memberships to the database.
    pub groups_claim: Option<String>,
}

/// What the callback needs from the start of a sign-in, kept in the session meanwhile. It
/// must be used once and then removed from the session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSignIn {
    /// Ties the callback to the browser that started the sign-in.
    pub state: String,
    /// Expected in the ID token, so a token issued for another sign-in is refused.
    pub nonce: String,
    pub pkce_verifier: String,
}

// All three values are secrets of an ongoing sign-in; they stay out of logs.
impl std::fmt::Debug for PendingSignIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingSignIn")
            .finish_non_exhaustive()
    }
}

/// A completed sign-in.
#[derive(Clone)]
pub struct SignedIn {
    /// What the provider says about the user, for the accounts database.
    pub identity: Identity,
    /// Kept in the session to hint the provider at sign-out.
    pub id_token: String,
}

// The ID token is a bearer credential for the provider's end session endpoint.
impl std::fmt::Debug for SignedIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignedIn")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
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

/// The provider as discovered: the client (with the provider's signing keys) and where to send
/// the browser to sign out.
struct Discovered {
    client: DiscoveredClient,
    end_session_endpoint: Option<Url>,
}

/// How long a failed discovery is remembered.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// The OpenID Connect provider, discovered on first use.
pub struct OpenId {
    settings: OpenIdSettings,
    http: reqwest::Client,
    /// The current discovery result, swapped when the provider is discovered again. Readers
    /// never wait for a discovery in progress.
    discovered: RwLock<Option<Arc<Discovered>>>,
    /// Serializes discoveries, and holds when and why the last one failed, so concurrent
    /// sign-in pages fail fast instead of each waiting for the provider's timeout.
    failure: Mutex<Option<(Instant, String)>>,
    retry_after: Duration,
}

impl OpenId {
    /// Prepares the client without any network access.
    pub fn new(settings: OpenIdSettings) -> Self {
        let http = reqwest::Client::builder()
            // Following redirects would expose the client to server-side request forgery.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            // Only fails when the TLS backend cannot initialize, which no retry can fix.
            .expect("the HTTP client could not be built");
        Self {
            settings,
            http,
            discovered: RwLock::new(None),
            failure: Mutex::new(None),
            retry_after: RETRY_AFTER,
        }
    }

    /// Changes how long a failed discovery is remembered before the next call tries again
    /// (5 seconds by default).
    #[must_use]
    pub fn retry_after(mut self, delay: Duration) -> Self {
        self.retry_after = delay;
        self
    }

    fn current(&self) -> Option<Arc<Discovered>> {
        self.discovered
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The discovered provider; discovery runs on the first call and again after a failure,
    /// but not within `retry_after` of the last failure.
    async fn discovered(&self) -> Result<Arc<Discovered>, OpenIdError> {
        if let Some(discovered) = self.current() {
            return Ok(discovered);
        }
        let mut failure = self.failure.lock().await;
        // Another caller may have finished while this one waited for the lock.
        if let Some(discovered) = self.current() {
            return Ok(discovered);
        }
        if let Some((at, reason)) = &*failure
            && at.elapsed() < self.retry_after
        {
            return Err(OpenIdError::Unavailable(reason.clone()));
        }
        self.discover(&mut failure).await
    }

    /// Discovers the provider again after a verification failed with the cached keys, which
    /// happens when the provider rotated them. `stale` is what the caller verified with: if
    /// another caller already refreshed, that result is used instead of another round trip.
    async fn refreshed(&self, stale: &Arc<Discovered>) -> Result<Arc<Discovered>, OpenIdError> {
        let mut failure = self.failure.lock().await;
        match self.current() {
            Some(current) if !Arc::ptr_eq(&current, stale) => Ok(current),
            _ => self.discover(&mut failure).await,
        }
    }

    async fn discover(
        &self,
        failure: &mut Option<(Instant, String)>,
    ) -> Result<Arc<Discovered>, OpenIdError> {
        match self.fetch().await {
            Ok(discovered) => {
                let discovered = Arc::new(discovered);
                *self
                    .discovered
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = Some(discovered.clone());
                *failure = None;
                Ok(discovered)
            }
            Err(error) => {
                // The previous keys stay in use: a provider that is briefly down must not
                // lock out sign-ins that the cached keys can still verify.
                *failure = Some((Instant::now(), error.clone()));
                Err(OpenIdError::Unavailable(error))
            }
        }
    }

    async fn fetch(&self) -> Result<Discovered, String> {
        let mut metadata = None;
        let mut first_error = None;
        for issuer in issuer_candidates(&self.settings.issuer) {
            let issuer = IssuerUrl::new(issuer).map_err(|error| error.to_string())?;
            match ProviderMetadataWithLogout::discover_async(issuer, &self.http).await {
                Ok(found) => {
                    metadata = Some(found);
                    break;
                }
                // Only an issuer mismatch is worth trying the other spelling for; the same
                // request to an unreachable provider would only wait for a second timeout.
                Err(error @ DiscoveryError::Validation(_)) => {
                    first_error.get_or_insert_with(|| error.to_string());
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        let metadata = metadata.ok_or_else(|| first_error.unwrap_or_default())?;
        let LogoutProviderMetadata {
            end_session_endpoint,
            additional_metadata: EmptyAdditionalProviderMetadata {},
        } = metadata.additional_metadata().clone();
        let client = Client::from_provider_metadata(
            metadata,
            ClientId::new(self.settings.client_id.clone()),
            None,
        )
        .set_redirect_uri(RedirectUrl::from_url(self.settings.redirect_url.clone()));
        Ok(Discovered {
            client,
            end_session_endpoint: end_session_endpoint.map(|url| url.url().clone()),
        })
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
    /// signature, issuer, audience, expiry, nonce and access token hash, then the required
    /// claim. When the signature cannot be checked with the cached keys, the provider is
    /// discovered again once, since it may have rotated them.
    pub async fn finish(
        &self,
        pending: PendingSignIn,
        code: &str,
        state: &str,
    ) -> Result<SignedIn, OpenIdError> {
        if state != pending.state {
            return Err(OpenIdError::StateMismatch);
        }
        let mut discovered = self.discovered().await?;
        let token = discovered
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|error| OpenIdError::Rejected(error.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|error| match error {
                RequestTokenError::Request(error) => OpenIdError::Unavailable(error.to_string()),
                other => OpenIdError::Rejected(other.to_string()),
            })?;
        let nonce = Nonce::new(pending.nonce);
        let (mut all, subject) = match verify(&discovered.client, &token, &nonce) {
            Err(Verification::Keys(_)) => {
                // The code is spent, so only the verification is retried, with fresh keys.
                discovered = self.refreshed(&discovered).await?;
                verify(&discovered.client, &token, &nonce)
            }
            other => other,
        }
        .map_err(|error| match error {
            Verification::Keys(reason) | Verification::Invalid(reason) => {
                OpenIdError::Rejected(reason)
            }
        })?;
        let client = &discovered.client;

        let required = self.settings.required_claim.as_deref();
        let required_value = self.settings.required_value.as_deref();
        let lacks_claim =
            required.is_some_and(|claim| !claims::allows(&all, claim, required_value));
        // Without these the username would be the subject, a long number with some providers.
        let lacks_profile = !["preferred_username", "email", "name"].iter().any(|key| {
            all.get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        });
        // A configured claim that is nowhere in the ID token may be in UserInfo.
        let lacks_extra = [
            &self.settings.administrator_claim,
            &self.settings.groups_claim,
        ]
        .into_iter()
        .flatten()
        .any(|claim| claims::lookup(&all, claim).is_none());
        // Set when UserInfo was needed and could not be read: a claim missing after that is
        // unknown, not absent, so nobody loses administration or groups over an outage.
        let mut incomplete = false;
        // One request covers all of them. A provider without a UserInfo endpoint leaves the
        // claims absent.
        if (lacks_claim || lacks_profile || lacks_extra)
            && let Ok(request) =
                client.user_info(token.access_token().clone(), Some(subject.clone()))
        {
            match request.request_async(&self.http).await {
                Ok(information) => {
                    let information: UserInfoClaims<ExtraClaims, CoreGenderClaim> = information;
                    for (key, value) in to_map(&information) {
                        all.entry(key).or_insert(value);
                    }
                }
                Err(UserInfoError::ClaimsVerification(error)) => {
                    // Typically a `sub` that is not the signed-in user's: nothing from that
                    // answer may grant access or name the user.
                    return Err(OpenIdError::Rejected(format!(
                        "the user information does not match the ID token: {error}"
                    )));
                }
                // Access cannot be decided without the claim, whatever went wrong (no answer,
                // an error status, an unreadable body); a missing profile only costs a nicer
                // name.
                Err(error) if lacks_claim => {
                    return Err(OpenIdError::Unavailable(error.to_string()));
                }
                Err(error) => {
                    incomplete = true;
                    tracing::warn!(%error, "reading the user information failed");
                }
            }
        }
        if let Some(claim) = required
            && !claims::allows(&all, claim, required_value)
        {
            return Err(OpenIdError::AccessDenied);
        }
        let identity = identity(&self.settings, &all, subject.as_str(), incomplete);
        let id_token = token
            .id_token()
            .map(ToString::to_string)
            .unwrap_or_default();
        Ok(SignedIn { identity, id_token })
    }

    /// Where to send the browser to end the session at the provider, when it supports it and
    /// has been discovered (a sign-in always discovers it first). Without an ID token, as after
    /// a sign-in refused for a missing role, `client_id` alone identifies recto, which
    /// RP-initiated logout allows; the provider may then ask the user to confirm.
    pub fn end_session_url(&self, id_token: Option<&str>, post_logout: &Url) -> Option<Url> {
        let mut url = self.current()?.end_session_endpoint.clone()?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(id_token) = id_token {
                query.append_pair("id_token_hint", id_token);
            }
            query
                .append_pair("client_id", &self.settings.client_id)
                .append_pair("post_logout_redirect_uri", post_logout.as_str());
        }
        Some(url)
    }
}

enum Verification {
    /// The signature could not be checked with the cached keys; fresh ones may help.
    Keys(String),
    Invalid(String),
}

/// Checks the ID token and returns all its claims and its subject.
fn verify(
    client: &DiscoveredClient,
    token: &ExtraTokenResponse,
    nonce: &Nonce,
) -> Result<(Map<String, Value>, openidconnect::SubjectIdentifier), Verification> {
    let invalid = |error: &dyn std::fmt::Display| Verification::Invalid(error.to_string());
    let id_token = token
        .id_token()
        .ok_or_else(|| Verification::Invalid("no ID token in the answer".to_owned()))?;
    let verifier = client.id_token_verifier();
    let claims = id_token
        .claims(&verifier, nonce)
        .map_err(|error| match error {
            ClaimsVerificationError::SignatureVerification(
                SignatureVerificationError::NoMatchingKey
                | SignatureVerificationError::CryptoError(_),
            ) => Verification::Keys(error.to_string()),
            other => invalid(&other),
        })?;
    if let Some(expected) = claims.access_token_hash() {
        let algorithm = id_token.signing_alg().map_err(|error| invalid(&error))?;
        let key = id_token
            .signing_key(&verifier)
            .map_err(|error| invalid(&error))?;
        let actual = AccessTokenHash::from_token(token.access_token(), algorithm, key)
            .map_err(|error| invalid(&error))?;
        if actual != *expected {
            return Err(Verification::Invalid(
                "the access token does not match the ID token".to_owned(),
            ));
        }
    }
    Ok((to_map(claims), claims.subject().clone()))
}

/// Builds the identity from the claims. `incomplete` says UserInfo failed, so a configured
/// claim that is missing is unknown (`None`) rather than absent (no administration, no groups).
fn identity(
    settings: &OpenIdSettings,
    all: &Map<String, Value>,
    subject: &str,
    incomplete: bool,
) -> Identity {
    let text = |key: &str| {
        all.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let administrator = settings.administrator_claim.as_deref().and_then(|claim| {
        if claims::lookup(all, claim).is_none() && incomplete {
            return None;
        }
        Some(claims::allows(
            all,
            claim,
            settings.administrator_value.as_deref(),
        ))
    });
    let groups =
        settings
            .groups_claim
            .as_deref()
            .and_then(|claim| match claims::lookup(all, claim) {
                Some(value) => Some(claims::names(value)),
                None if incomplete => None,
                None => Some(Vec::new()),
            });
    Identity {
        // As configured, so the same provider always yields the same string even when the
        // discovered issuer is spelled with or without a trailing slash.
        issuer: settings.issuer.as_str().to_owned(),
        subject: subject.to_owned(),
        preferred_username: text("preferred_username"),
        email: text("email"),
        name: text("name"),
        administrator,
        groups,
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
