//! Reading request bodies that devices send, bounded in size and in time, whatever they
//! announce: chunked bodies announce nothing, and a client trickling bytes must not hold a
//! request open.

use std::{future::poll_fn, pin::Pin, time::Duration};

use axum::{
    body::{Body, HttpBody},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

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
