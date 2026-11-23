//! Reading request bodies bounded in size and in time, whatever they announce: chunked bodies
//! announce nothing, and a client trickling bytes must not hold a request open. Devices' bodies
//! are read with [`fill`] and [`read_all`]; the web's go through [`bounded`].

use std::{future::poll_fn, pin::Pin, time::Duration};

use axum::{
    body::{Body, HttpBody},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::timeout::{DeadlineBody, TimeoutBody};

/// How many times the read timeout a request carrying a document may take in all: 10 minutes
/// with the 30 seconds default, about 85 kB/s for a 50 MiB document. A request without one must
/// arrive within the read timeout itself.
pub(crate) const DOCUMENT_DEADLINE_FACTOR: u32 = 20;

/// The slowest upload the print form waits for, in bytes per second, so that a larger
/// `RECTO_MAX_UPLOAD_SIZE` also gives a document longer to arrive.
const SLOWEST_UPLOAD: u64 = 64 * 1024;

/// The path of the print form, the one web form that carries a document.
const PRINT_FORM: &str = "/print";

/// Bounds the body of a web request in time, as devices' are: each piece must arrive within
/// `timeout` of the last, and the whole body within `timeout`. The print form has
/// [`DOCUMENT_DEADLINE_FACTOR`] times it, or as long as [`SLOWEST_UPLOAD`] takes for
/// `max_upload` bytes when that is longer. A body that runs out of time fails to read with a
/// [`TimeoutError`](tower_http::timeout::TimeoutError), which [`timed_out`] finds. Without this, a few slow posts, signed in or not (the
/// sign-in form), would hold connections and their buffers for as long as they trickle.
pub(crate) async fn bounded(
    State((timeout, max_upload)): State<(Duration, usize)>,
    request: Request,
    next: Next,
) -> Response {
    let deadline = if request.uri().path() == PRINT_FORM {
        document_deadline(timeout, max_upload)
    } else {
        timeout
    };
    let request =
        request.map(|body| Body::new(DeadlineBody::new(deadline, TimeoutBody::new(timeout, body))));
    next.run(request).await
}

/// How long the print form's whole body may take (see [`bounded`]).
fn document_deadline(timeout: Duration, max_upload: usize) -> Duration {
    (timeout * DOCUMENT_DEADLINE_FACTOR)
        .max(Duration::from_secs(max_upload as u64 / SLOWEST_UPLOAD))
}

/// Whether reading a body failed because it ran out of time in [`bounded`], whatever wraps the
/// error on its way up.
pub(crate) fn timed_out(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |error| error.source())
        .any(|error| error.is::<tower_http::timeout::TimeoutError>())
}

/// Whether the body announces more than `limit` bytes, so it can be refused before any is read.
pub(crate) fn announced_over(headers: &HeaderMap, limit: usize) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
}

/// Why a body could not be read.
pub(crate) enum Unread {
    TooLarge,
    Failed,
    TimedOut,
}

impl IntoResponse for Unread {
    fn into_response(self) -> Response {
        match self {
            Unread::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Unread::Failed => StatusCode::BAD_REQUEST,
            Unread::TimedOut => StatusCode::REQUEST_TIMEOUT,
        }
        .into_response()
    }
}

/// Reads the body into `buffer` until it holds `wanted` bytes or the body ends, refusing more
/// than `limit` in all. Each piece must arrive within `idle` of the last; callers bound the
/// whole read too.
pub(crate) async fn fill(
    body: &mut Body,
    buffer: &mut Vec<u8>,
    wanted: usize,
    limit: usize,
    idle: Duration,
) -> Result<(), Unread> {
    // What an earlier read took may already be over a limit lowered since.
    if buffer.len() > limit {
        return Err(Unread::TooLarge);
    }
    while buffer.len() < wanted {
        let next = poll_fn(|context| Pin::new(&mut *body).poll_frame(context));
        let frame = tokio::time::timeout(idle, next)
            .await
            .map_err(|_| Unread::TimedOut)?;
        let Some(frame) = frame else {
            return Ok(());
        };
        let Ok(data) = frame.map_err(|_| Unread::Failed)?.into_data() else {
            continue;
        };
        if buffer.len().saturating_add(data.len()) > limit {
            return Err(Unread::TooLarge);
        }
        buffer.extend_from_slice(&data);
    }
    Ok(())
}

/// The whole body, at most `limit` bytes, all of it within `timeout`.
pub(crate) async fn read_all(
    mut body: Body,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, Unread> {
    let mut buffer = Vec::new();
    let read = fill(&mut body, &mut buffer, usize::MAX, limit, timeout);
    tokio::time::timeout(timeout, read)
        .await
        .unwrap_or(Err(Unread::TimedOut))?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_larger_upload_limit_gives_documents_longer() {
        const MIB: usize = 1024 * 1024;
        let timeout = Duration::from_secs(30);
        assert_eq!(
            document_deadline(timeout, 50 * MIB),
            Duration::from_secs(800)
        );
        assert_eq!(
            document_deadline(timeout, 10 * MIB),
            Duration::from_secs(600)
        );
        assert_eq!(
            document_deadline(timeout, 200 * MIB),
            Duration::from_secs(3200)
        );
    }
}
