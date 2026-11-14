//! A route signing anyone in, for this package's integration tests. Compiled only with the
//! `testing` feature, which only the package's dev-dependency on itself enables: no build of the
//! binary can contain it.

use axum::{Router, extract::Query, http::StatusCode, routing::get};
use recto_authentication::User;
use serde::Deserialize;
use tower_sessions::Session;

#[derive(Deserialize)]
struct SignIn {
    subject: String,
    username: String,
    name: Option<String>,
}

/// `GET /testing/sign-in?subject=..&username=..[&name=..]`, through the same session code as
/// the provider's callback.
pub fn router() -> Router {
    Router::new().route("/testing/sign-in", get(sign_in))
}

async fn sign_in(session: Session, Query(query): Query<SignIn>) -> StatusCode {
    let user = User {
        subject: query.subject,
        username: query.username,
        name: query.name,
    };
    match crate::session::sign_in(&session, &user, "").await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
