// SPDX-License-Identifier: Apache-2.0
//! One error type for every call, from an extension or from the AWS SDK underneath (protocol §6).

use std::fmt;
use std::time::Duration;

use aws_sdk_s3::config::http::HttpResponse;
use aws_sdk_s3::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server answered with an error status.
    #[error("{0}")]
    Service(ServiceError),
    /// No answer: the connection failed, timed out or broke. `sent` says whether the request may
    /// have reached the server, and so may have taken effect.
    #[error("request failed: {message}")]
    Transport { message: String, sent: bool },
    /// An answer this SDK could not read.
    #[error("unexpected response: {0}")]
    Decode(String),
    /// Refused before anything was sent: a bad argument or configuration.
    #[error("{0}")]
    Invalid(String),
}

/// An error the server answered with: the status, and the S3 error body's code and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
    /// On a `412`, the object's current version, when it exists.
    pub current_version_id: Option<String>,
    /// `Retry-After`, when the server sent one.
    pub retry_after: Option<Duration>,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status, self.code)?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        if let Some(id) = &self.request_id {
            write!(f, " (request {id})")?;
        }
        Ok(())
    }
}

impl Error {
    /// The HTTP status of a service error.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Service(s) => Some(s.status),
            _ => None,
        }
    }

    /// The S3 error code of a service error, for example `PreconditionFailed`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Service(s) => Some(&s.code),
            _ => None,
        }
    }

    /// On a `412`: the object's current version, when it exists. Re-read from it and retry.
    pub fn current_version_id(&self) -> Option<&str> {
        match self {
            Error::Service(s) => s.current_version_id.as_deref(),
            _ => None,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Error::Service(s) => s.request_id.as_deref(),
            _ => None,
        }
    }

    /// `404` of any kind: no such drive, key or version.
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }

    pub(crate) fn decode(what: impl fmt::Display) -> Error {
        Error::Decode(what.to_string())
    }
}

/// The code for an error answered without a body, as to HEAD.
pub(crate) fn code_for_status(status: u16) -> &'static str {
    match status {
        400 => "BadRequest",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        409 => "Conflict",
        410 => "Gone",
        412 => "PreconditionFailed",
        413 => "EntityTooLarge",
        416 => "InvalidRange",
        500 => "InternalError",
        501 => "NotImplemented",
        503 => "ServiceUnavailable",
        _ => "Unknown",
    }
}

/// The error a response with `status` carries: its XML body's code, message and request id, and
/// the headers that go with them.
pub(crate) fn service_error(status: u16, headers: &http::HeaderMap, body: &[u8]) -> ServiceError {
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let mut code = None;
    let mut message = String::new();
    let mut request_id = None;
    if let Ok(text) = std::str::from_utf8(body)
        && let Ok(doc) = roxmltree::Document::parse(text)
    {
        let child = |name: &str| doc.root_element().children().find(|n| n.has_tag_name(name)).and_then(|n| n.text()).map(str::to_owned);
        if doc.root_element().has_tag_name("Error") {
            code = child("Code");
            message = child("Message").unwrap_or_default();
            request_id = child("RequestId");
        }
    }
    if code.is_none() && message.is_empty() && !body.is_empty() {
        message = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
    }
    ServiceError {
        status,
        code: code.unwrap_or_else(|| code_for_status(status).to_owned()),
        message,
        request_id: request_id.or_else(|| header("x-amz-request-id")),
        current_version_id: if status == 412 { header("x-amz-version-id") } else { None },
        retry_after: header("retry-after").and_then(|s| s.trim().parse::<u64>().ok()).map(Duration::from_secs),
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Error {
        // A failure to connect sends nothing; anything later may have reached the server.
        let mut message = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            message.push_str(&format!(": {s}"));
            source = s.source();
        }
        Error::Transport { message, sent: !e.is_connect() }
    }
}

/// An error of the AWS SDK, as this type.
pub(crate) fn from_s3<E>(e: SdkError<E, HttpResponse>) -> Error
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    if let Some(raw) = e.raw_response() {
        let status = raw.status().as_u16();
        if status >= 300 {
            let header = |n: &str| raw.headers().get(n).map(str::to_owned);
            let (code, message) = match e.as_service_error() {
                Some(s) => (s.code().map(str::to_owned), s.message().map(str::to_owned)),
                None => (None, None),
            };
            return Error::Service(ServiceError {
                status,
                code: code.unwrap_or_else(|| code_for_status(status).to_owned()),
                message: message.unwrap_or_default(),
                request_id: header("x-amz-request-id"),
                current_version_id: if status == 412 { header("x-amz-version-id") } else { None },
                retry_after: header("retry-after").and_then(|s| s.trim().parse::<u64>().ok()).map(Duration::from_secs),
            });
        }
    }
    let message = DisplayErrorContext(&e).to_string();
    match e {
        SdkError::ConstructionFailure(_) => Error::Invalid(message),
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) => Error::Transport { message, sent: true },
        _ => Error::Decode(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_errors_are_read_from_the_xml_body_and_headers() {
        let mut h = http::HeaderMap::new();
        h.insert("x-amz-version-id", "7.0".parse().unwrap());
        h.insert("x-amz-request-id", "header-id".parse().unwrap());
        h.insert("retry-after", "2".parse().unwrap());
        let body = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>PreconditionFailed</Code><Message>no &amp; no</Message><RequestId>body-id</RequestId></Error>";
        let e = service_error(412, &h, body);
        assert_eq!(e.code, "PreconditionFailed");
        assert_eq!(e.message, "no & no");
        assert_eq!(e.request_id.as_deref(), Some("body-id"));
        assert_eq!(e.current_version_id.as_deref(), Some("7.0"));
        assert_eq!(e.retry_after, Some(Duration::from_secs(2)));
        assert_eq!(e.to_string(), "412 PreconditionFailed: no & no (request body-id)");
        // Not a 412: the version header is not a current version.
        assert_eq!(service_error(200, &h, body).current_version_id, None);
    }

    #[test]
    fn errors_without_a_body_get_a_code_from_their_status() {
        let e = service_error(404, &http::HeaderMap::new(), b"");
        assert_eq!((e.code.as_str(), e.message.as_str(), e.request_id), ("NotFound", "", None));
        let e = service_error(502, &http::HeaderMap::new(), b"bad gateway");
        assert_eq!((e.code.as_str(), e.message.as_str()), ("Unknown", "bad gateway"));
    }
}
