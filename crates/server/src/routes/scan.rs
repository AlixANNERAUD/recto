//! Finished scans: shown inline (previews), downloaded, or saved to the output folder. A result
//! belongs to the id of the user who scanned it; anyone else's, an expired one and an
//! unknown one all answer 404 alike.

use std::{sync::Arc, time::SystemTime};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use recto_app::{i18n, messages, state::AppState};
use recto_authentication::User;
use recto_scanning::{ScanError, ScanResult, save_to_folder};
use serde_json::json;
use uuid::Uuid;

/// `GET /scan/results/{id}`: the file itself, for previews.
pub async fn show(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve(&state, &user, &id, &headers, "inline").await
}

/// `GET /scan/results/{id}/download`: the file under its `scan-YYYYMMDD-HHMMSS.ext` name.
pub async fn download(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve(&state, &user, &id, &headers, "attachment").await
}

/// `POST /scan/results/{id}/save`: writes the file to the output folder. Answers
/// `{ "path": "<saved file>" }`, or `{ "error": "<message>" }` with 404 (no such result, or no
/// folder configured) or 500 (the folder cannot be written).
pub async fn save(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let locale = i18n::from_headers(&headers);
    let failure = |status: StatusCode, message: String| (status, Json(json!({ "error": message })));
    let Some(dir) = state.output_dir.clone() else {
        let message = messages::save_disabled(locale).to_owned();
        return failure(StatusCode::NOT_FOUND, message).into_response();
    };
    let not_found = || {
        let message = messages::scan_result_not_found(locale).to_owned();
        failure(StatusCode::NOT_FOUND, message).into_response()
    };
    let Some(result) = find(&state, &user, &id) else {
        return not_found();
    };
    // A result that cannot be read is gone for the user; the store's own path stays private.
    let Some(bytes) = read(&state, &result).await else {
        return not_found();
    };
    let saved =
        tokio::task::spawn_blocking(move || save_to_folder(&dir, &result.file_name, &bytes))
            .await
            .unwrap_or(Err(ScanError::Interrupted));
    match saved {
        Ok(path) => {
            tracing::info!(user = user.username, path = %path.display(), "scan saved");
            Json(json!({ "path": path })).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "saving a scan failed");
            let message = messages::scan_error(&error, locale);
            failure(StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
        }
    }
}

/// The bytes of a stored result, `None` when its file went missing under the index (a cleaned
/// temporary directory).
async fn read(state: &AppState, result: &ScanResult) -> Option<Vec<u8>> {
    let results = Arc::clone(&state.results);
    let result = result.clone();
    match tokio::task::spawn_blocking(move || results.read(&result)).await {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(error)) => {
            tracing::warn!(%error, "reading a scan result failed");
            None
        }
        Err(_) => None,
    }
}

/// The user's result `id`, unless it has expired.
fn find(state: &AppState, user: &User, id: &str) -> Option<ScanResult> {
    let id = Uuid::parse_str(id).ok()?;
    let result = state.results.get(&user.id.to_string(), id)?;
    (!state.results.is_expired(&result, SystemTime::now())).then_some(result)
}

async fn serve(
    state: &AppState,
    user: &User,
    id: &str,
    headers: &HeaderMap,
    disposition: &str,
) -> Response {
    let not_found = || {
        let locale = i18n::from_headers(headers);
        (
            StatusCode::NOT_FOUND,
            messages::scan_result_not_found(locale),
        )
            .into_response()
    };
    let Some(result) = find(state, user, id) else {
        return not_found();
    };
    let Some(bytes) = read(state, &result).await else {
        return not_found();
    };
    // The name is recto's own `scan-YYYYMMDD-HHMMSS.ext`, safe inside quotes.
    let disposition = format!("{disposition}; filename=\"{}\"", result.file_name);
    let mut response = bytes.into_response();
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(result.format.mime()),
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        response_headers.insert(header::CONTENT_DISPOSITION, value);
    }
    // Scans are private documents: no shared cache keeps them, and browsers take the type as
    // given.
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
