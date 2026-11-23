//! IPP over HTTP to CUPS, shared by the print client ([`crate::Cups`]) and the administration
//! client ([`crate::CupsAdmin`]), which each turn a [`Failure`] into their own error.

use std::io::Cursor;
use std::time::Duration;

use ipp::model::DelimiterTag;
use ipp::parser::IppParser;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;

use crate::cups::string;

/// Long enough for a busy CUPS, short enough that a page never hangs on a dead one.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A client giving up on connecting, and on a request, after `timeout`. The workspace reqwest
/// (rustls with ring) does the HTTP instead of the client bundled with `ipp`, which would pull
/// in a second TLS stack built on aws-lc.
pub(crate) fn client(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
}

/// Why an IPP request got no IPP answer.
pub(crate) enum Failure {
    /// The connection never opened, so CUPS never got the request.
    Unsent(reqwest::Error),
    /// CUPS, or a proxy in front of it, answered with this HTTP status instead.
    Status(reqwest::StatusCode),
    /// The request reached CUPS, or may have, and no usable answer came back.
    Unanswered(String),
}

/// Posts `post`, which holds an IPP request, and parses CUPS's answer.
pub(crate) async fn exchange(post: reqwest::RequestBuilder) -> Result<IppRequestResponse, Failure> {
    let response = post
        .header("content-type", "application/ipp")
        .send()
        .await
        .map_err(|error| {
            // Only a connection that never opened is sure not to have carried the request: a
            // timeout or a broken connection may come after CUPS read it.
            if error.is_connect() || error.is_builder() {
                Failure::Unsent(error)
            } else {
                Failure::Unanswered(error_chain(&error))
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(Failure::Status(status));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| Failure::Unanswered(error_chain(&error)))?;
    IppParser::new(IppReader::new(Cursor::new(bytes)))
        .parse()
        .map_err(|error| Failure::Unanswered(format!("unreadable CUPS answer: {error}")))
}

/// The error and its causes, joined, so the reason is not lost behind "error sending request".
pub(crate) fn error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// The `status-message` CUPS sent with a refusal, if any.
pub(crate) fn status_message(response: &IppRequestResponse) -> Option<String> {
    response
        .attributes()
        .groups_of(DelimiterTag::OperationAttributes)
        .find_map(|group| string(group, "status-message"))
}
