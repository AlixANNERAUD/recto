//! The held jobs of the Jobs page: documents computers and phones sent, waiting for their owner.
//! `GET /jobs/held/{id}/document` shows one, `POST /jobs/held/{id}/release` prints it through
//! the print path, and `POST /jobs/held/{id}/delete` deletes it. The forms are plain posts
//! answered with a redirect, so they work without JavaScript.
//!
//! A held job is its owner's only: anyone else, administrators included, gets the not-found
//! page, as for a job that does not exist. Releasing claims the job first, so of two releases
//! at once exactly one prints; the print path marks it released in the transaction that counts
//! it (see `print_document`).

use std::sync::Arc;

use axum::{
    Extension, Form, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use recto_app::{
    pages::sign_in::SignInMethods,
    refusal::Refusal,
    state::{AppState, User},
};
use recto_domain::held::{self, HeldJob, HeldState};
use recto_printing::DocumentFormat;
use serde::Deserialize;

use super::{
    print::{Choices, Document, HeldClaim, Rejected, Submitted, print_document},
    render,
};
use crate::{session, state::AuthState};

#[derive(Clone)]
struct HeldRoutes {
    app: AppState,
    methods: SignInMethods,
}

/// The routes, behind the session; the cross-site check is the router's.
pub fn router(app: AppState, auth: &AuthState) -> Router {
    let state = HeldRoutes {
        app,
        methods: auth.sign_in_methods(),
    };
    Router::new()
        .route("/jobs/held/{id}/document", get(document))
        .route("/jobs/held/{id}/release", post(release))
        .route("/jobs/held/{id}/delete", post(delete))
        .route_layer(middleware::from_fn(session::require_user))
        .with_state(state)
}

/// The fields of the release form; empty ones mean the printer's default.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ReleaseFields {
    printer: String,
    copies: String,
    sides: String,
    color_mode: String,
    page_ranges: String,
}

impl ReleaseFields {
    fn choices(self) -> Choices {
        let given = |value: String| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        Choices {
            printer: self.printer.trim().to_owned(),
            copies: given(self.copies),
            sides: given(self.sides),
            color_mode: given(self.color_mode),
            page_ranges: given(self.page_ranges),
        }
    }
}

/// `GET /jobs/held/{id}/document`: the document, shown inline. Not sandboxed, as scans are not:
/// a sandbox stops Chrome's PDF viewer.
async fn document(
    State(state): State<HeldRoutes>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    parts: Parts,
) -> Response {
    let job = match own(&state, &user, &id).await {
        Ok(Some(job)) => job,
        Ok(None) => return not_found(&state, parts).await,
        Err(status) => return status.into_response(),
    };
    let (Some(file), Some(format)) = (job.file.clone(), job.stored_format.clone()) else {
        return not_found(&state, parts).await;
    };
    let files = Arc::clone(&state.app.held);
    let read = tokio::task::spawn_blocking(move || files.read(&file)).await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return not_found(&state, parts).await;
        }
        Ok(Err(error)) => {
            tracing::error!(%error, job = job.id, "reading a held document failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(error) => {
            tracing::error!(%error, job = job.id, "reading a held document failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let content_type =
        HeaderValue::from_str(&format).unwrap_or(HeaderValue::from_static("application/pdf"));
    (
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("inline"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// `POST /jobs/held/{id}/release`: answers `303 /jobs?submitted=<id>` (with `&over=` as the
/// print form), `/jobs?error=quota&rule=<id>`, or `/jobs?error=<code>`.
async fn release(
    State(state): State<HeldRoutes>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    parts: Parts,
    Form(fields): Form<ReleaseFields>,
) -> Response {
    let job = match own(&state, &user, &id).await {
        Ok(Some(job)) => job,
        Ok(None) => return not_found(&state, parts).await,
        Err(status) => return status.into_response(),
    };
    let app = &state.app;
    let claimed = match held::claim(&app.database, user.id, job.id, chrono::Utc::now()).await {
        Ok(Some(claimed)) => claimed,
        // Another release took it, it was deleted, or it expired.
        Ok(None) => return refused(Refusal::HeldGone),
        Err(error) => {
            tracing::error!(%error, job = job.id, "claiming a held job failed");
            return refused(Refusal::Unavailable);
        }
    };
    let claim = HeldClaim {
        id: claimed.id,
        claim: claimed.claim,
    };
    let Some(file) = claimed.file.clone() else {
        give_back(app, &claim, Refusal::HeldGone).await;
        return refused(Refusal::HeldGone);
    };
    let files = Arc::clone(&app.held);
    let reading = file.clone();
    let read = tokio::task::spawn_blocking(move || files.read(&reading)).await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        failed => {
            tracing::error!(?failed, job = claim.id, "reading a held document failed");
            give_back(app, &claim, Refusal::Unavailable).await;
            return refused(Refusal::Unavailable);
        }
    };
    // Checked when it arrived; read again from the content, never trusted from the row.
    let Some(format) = DocumentFormat::detect(&bytes) else {
        give_back(app, &claim, Refusal::UnsupportedFormat).await;
        return refused(Refusal::UnsupportedFormat);
    };
    let document = Document {
        name: claimed.name.clone(),
        bytes: bytes.into(),
        format,
    };
    let choices = fields.choices();
    if choices.printer.is_empty() {
        give_back(app, &claim, Refusal::InvalidForm).await;
        return refused(Refusal::InvalidForm);
    }
    match print_document(app, &user, document, &choices, Some(claim)).await {
        Ok(Submitted { job, over }) => {
            // Printed: the document has no reason to stay.
            let files = Arc::clone(&app.held);
            let removed = tokio::task::spawn_blocking(move || files.remove(&file)).await;
            if !matches!(removed, Ok(Ok(()))) {
                tracing::warn!(job = claimed.id, "removing a released document failed");
            }
            tracing::info!(
                user = user.username,
                held_job = claimed.id,
                job,
                printer = choices.printer,
                "held job released"
            );
            let mut to = format!("/jobs?submitted={job}");
            if !over.is_empty() {
                let over: Vec<String> = over.iter().map(i64::to_string).collect();
                to.push_str(&format!("&over={}", over.join(",")));
            }
            Redirect::to(&to).into_response()
        }
        Err(Rejected::Quota(rule)) => {
            tracing::info!(user = user.username, rule, "held job refused by a quota");
            Redirect::to(&format!(
                "/jobs?error={}&rule={rule}",
                Refusal::Quota.code()
            ))
            .into_response()
        }
        Err(Rejected::Refusal(refusal)) => refused(refusal),
    }
}

/// `POST /jobs/held/{id}/delete`: answers `303 /jobs?done=deleted`. A job being printed stays.
async fn delete(
    State(state): State<HeldRoutes>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    parts: Parts,
) -> Response {
    let job = match own(&state, &user, &id).await {
        Ok(Some(job)) => job,
        Ok(None) => return not_found(&state, parts).await,
        Err(status) => return status.into_response(),
    };
    let app = &state.app;
    match held::delete(&app.database, user.id, job.id).await {
        Ok(Some(deleted)) => {
            if let Some(file) = deleted.file {
                let files = Arc::clone(&app.held);
                let removed = tokio::task::spawn_blocking(move || files.remove(&file)).await;
                if !matches!(removed, Ok(Ok(()))) {
                    // The orphan sweep removes it.
                    tracing::warn!(job = job.id, "removing a deleted held document failed");
                }
            }
            tracing::info!(user = user.username, held_job = job.id, "held job deleted");
            Redirect::to("/jobs?done=deleted").into_response()
        }
        Ok(None) => refused(Refusal::HeldGone),
        Err(error) => {
            tracing::error!(%error, job = job.id, "deleting a held job failed");
            refused(Refusal::Unavailable)
        }
    }
}

/// The user's held job `id`, if it is theirs and has a document; `Err` is the status to answer
/// when the database fails.
async fn own(state: &HeldRoutes, user: &User, id: &str) -> Result<Option<HeldJob>, StatusCode> {
    let Ok(id) = id.parse::<i64>() else {
        return Ok(None);
    };
    match held::get(&state.app.database, user.id, id).await {
        Ok(job) => Ok(job.filter(|job| job.state != HeldState::Incoming)),
        Err(error) => {
            tracing::error!(%error, job = id, "reading a held job failed");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// Puts a claimed job back, for a refusal before the print path.
async fn give_back(app: &AppState, claim: &HeldClaim, refusal: Refusal) {
    if let Err(error) = held::unclaim(
        &app.database,
        claim.id,
        claim.claim,
        Some(refusal.code()),
        chrono::Utc::now(),
    )
    .await
    {
        tracing::error!(%error, job = claim.id, "returning a held job failed");
    }
}

fn refused(refusal: Refusal) -> Response {
    Redirect::to(&format!("/jobs?error={}", refusal.code())).into_response()
}

async fn not_found(state: &HeldRoutes, parts: Parts) -> Response {
    render(
        &state.app,
        state.methods,
        parts,
        StatusCode::NOT_FOUND,
        || {},
    )
    .await
}
