//! The held jobs of the Jobs page: documents computers and phones sent, waiting for their owner.
//! `GET /jobs/held/{id}/document` shows one, `POST /jobs/held/{id}/release` prints it through
//! the print path, and `POST /jobs/held/{id}/delete` deletes it. The forms are plain posts
//! answered with a redirect, so they work without JavaScript.
//!
//! A held job is its owner's only: anyone else, administrators included, gets the not-found
//! page, as for a job that does not exist. Releasing claims the job first, so of two releases
//! at once exactly one prints; the print path marks it released in the transaction that counts
//! it (see `print_document`).

use axum::{
    Extension, Form, Router,
    body::Bytes,
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
    not_found,
    print::{
        Choices, Document, HeldClaim, Rejected, Submitted, give_back, location, print_document,
        refused_location,
    },
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
        Ok(None) => return not_found(&state.app, state.methods, parts).await,
        Err(status) => return status.into_response(),
    };
    // Only while it waits: a released job is never offered again, and an expired one waits
    // only for the sweep.
    let waiting = matches!(job.state, HeldState::Held | HeldState::Releasing)
        && job.expires_at > chrono::Utc::now();
    let (true, Some(file), Some(format)) = (waiting, job.file.clone(), job.stored_format.clone())
    else {
        return not_found(&state.app, state.methods, parts).await;
    };
    let bytes = match state
        .app
        .held
        .blocking(move |files| files.read(&file))
        .await
    {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return not_found(&state.app, state.methods, parts).await;
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
        Ok(None) => return not_found(&state.app, state.methods, parts).await,
        Err(status) => return status.into_response(),
    };
    // In a task of its own: the request is dropped when the browser goes away, and a release cut
    // short after it was counted, before CUPS has the document, would leave it released and
    // never printed.
    let app = state.app.clone();
    let releasing = user.clone();
    let id = job.id;
    let choices = fields.choices();
    let outcome =
        tokio::spawn(async move { release_held(&app, &releasing, id, choices, None).await });
    let outcome = match outcome.await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(%error, job = id, "releasing a held job failed");
            Err(Rejected::Refusal(Refusal::Unavailable))
        }
    };
    Redirect::to(&location(&outcome, "/jobs")).into_response()
}

/// Releases the user's held job `id` to the printer of `choices`: claims it, so of two releases
/// at once exactly one prints, then prints it through [`print_document`], which marks it
/// released in the transaction that counts it. The Jobs page and printing directly both come
/// here. A refusal leaves the job held with why.
///
/// `in_hand` is the document, when the caller has it already, as printing directly does right
/// after holding it: it is not read back from disk then. The pages recto counted when the job
/// arrived are not counted again either.
pub(crate) async fn release_held(
    app: &AppState,
    user: &User,
    id: i64,
    choices: Choices,
    in_hand: Option<Bytes>,
) -> Result<Submitted, Rejected> {
    let claimed = match held::claim(&app.database, user.id, id, chrono::Utc::now()).await {
        Ok(Some(claimed)) => claimed,
        // Another release took it, it was deleted, or it expired.
        Ok(None) => return Err(Refusal::HeldGone.into()),
        Err(error) => {
            tracing::error!(%error, job = id, "claiming a held job failed");
            return Err(Refusal::Unavailable.into());
        }
    };
    let claim = HeldClaim {
        id: claimed.id,
        claim: claimed.claim,
    };
    let Some(file) = claimed.file.clone() else {
        give_back(app, &claim, Refusal::HeldGone).await;
        return Err(Refusal::HeldGone.into());
    };
    let bytes = match in_hand {
        Some(bytes) => bytes,
        None => {
            let reading = file.clone();
            match app.held.blocking(move |files| files.read(&reading)).await {
                Ok(bytes) => Bytes::from(bytes),
                Err(error) => {
                    tracing::error!(%error, job = claim.id, "reading a held document failed");
                    give_back(app, &claim, Refusal::Unavailable).await;
                    return Err(Refusal::Unavailable.into());
                }
            }
        }
    };
    // Checked when it arrived; read again from the content, never trusted from the row.
    let Some(format) = DocumentFormat::detect(&bytes) else {
        give_back(app, &claim, Refusal::UnsupportedFormat).await;
        return Err(Refusal::UnsupportedFormat.into());
    };
    let document = Document {
        name: claimed.name.clone(),
        bytes,
        format,
        // Counted by recto when the job arrived, from this same document.
        pages: claimed
            .document_pages
            .and_then(|pages| u32::try_from(pages).ok()),
    };
    if choices.printer.is_empty() {
        give_back(app, &claim, Refusal::InvalidForm).await;
        return Err(Refusal::InvalidForm.into());
    }
    let printed = print_document(app, user, document, &choices, Some(claim)).await;
    // Printed, or perhaps printed: never offered again, so the document has no reason to stay.
    if matches!(
        printed,
        Ok(_) | Err(Rejected::Refusal(Refusal::Unconfirmed))
    ) {
        let removed = app.held.blocking(move |files| files.remove(&file)).await;
        if removed.is_err() {
            tracing::warn!(job = claimed.id, "removing a released document failed");
        }
    }
    match printed {
        Ok(submitted) => {
            tracing::info!(
                user = user.username,
                held_job = claimed.id,
                job = submitted.job,
                printer = choices.printer,
                "held job released"
            );
            Ok(submitted)
        }
        Err(Rejected::Quota(rule)) => {
            tracing::info!(user = user.username, rule, "held job refused by a quota");
            Err(Rejected::Quota(rule))
        }
        Err(rejected) => Err(rejected),
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
        Ok(None) => return not_found(&state.app, state.methods, parts).await,
        Err(status) => return status.into_response(),
    };
    let app = &state.app;
    match held::delete(&app.database, user.id, job.id).await {
        Ok(Some(deleted)) => {
            if let Some(file) = deleted.file {
                let removed = app.held.blocking(move |files| files.remove(&file)).await;
                if removed.is_err() {
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

fn refused(refusal: Refusal) -> Response {
    Redirect::to(&refused_location("/jobs", refusal)).into_response()
}
