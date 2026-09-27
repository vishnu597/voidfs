// SPDX-License-Identifier: Apache-2.0
//! S3 errors: the standard XML error body (protocol §6).

use axum::body::Body;
use axum::response::Response;
use http::StatusCode;
use voidfs_core::ops::OpError;

use super::util::xml_escape;
use crate::pool::CommitError;
use crate::sigv4::AuthError;

#[derive(Debug)]
pub struct S3Error {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub headers: Vec<(&'static str, String)>,
}

impl S3Error {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        S3Error { status: StatusCode::from_u16(status).unwrap(), code, message: message.into(), headers: Vec::new() }
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        S3Error::new(400, "InvalidArgument", message)
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        S3Error::new(501, "NotImplemented", what)
    }

    pub fn no_bucket() -> Self {
        S3Error::new(404, "NoSuchBucket", "The specified drive does not exist")
    }

    pub fn no_key() -> Self {
        S3Error::new(404, "NoSuchKey", "The specified key does not exist")
    }

    pub fn denied(message: impl Into<String>) -> Self {
        S3Error::new(403, "AccessDenied", message)
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        S3Error::new(500, "InternalError", e.to_string())
    }

    pub fn auth(e: AuthError) -> Self {
        let (code, status) = e.s3_code();
        S3Error::new(status, code, e.to_string())
    }

    pub fn into_response(self, request_id: &str, head: bool) -> Response {
        let body = if head {
            Body::empty()
        } else {
            Body::from(format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{}</Code><Message>{}</Message><RequestId>{}</RequestId></Error>",
                self.code,
                xml_escape(&self.message),
                request_id
            ))
        };
        let mut b = Response::builder().status(self.status).header("content-type", "application/xml");
        for (k, v) in self.headers {
            b = b.header(k, v);
        }
        b.body(body).unwrap()
    }
}

impl From<OpError> for S3Error {
    fn from(e: OpError) -> Self {
        match e {
            OpError::NoSuchKey => S3Error::no_key(),
            OpError::NoSuchVersion => S3Error::new(404, "NoSuchVersion", "The specified version does not exist"),
            OpError::PathConflict => S3Error::new(409, "PathConflict", e.to_string()),
            OpError::PreconditionFailed { current } => {
                let err = S3Error::new(412, "PreconditionFailed", "At least one of the preconditions you specified did not hold");
                match current {
                    Some(v) => err.header("x-amz-version-id", v.to_string()),
                    None => err,
                }
            }
            OpError::InvalidArgument(m) => S3Error::invalid(m),
            OpError::TooLarge(m) => S3Error::new(413, "EntityTooLarge", m),
        }
    }
}

impl From<CommitError> for S3Error {
    fn from(e: CommitError) -> Self {
        match e {
            CommitError::Op(op) => op.into(),
            CommitError::Retry => S3Error::new(503, "SlowDown", "The drive is busy; retry").header("retry-after", "1"),
            CommitError::Other(e) => S3Error::internal(format!("{e:#}")),
        }
    }
}

impl From<anyhow::Error> for S3Error {
    fn from(e: anyhow::Error) -> Self {
        S3Error::internal(format!("{e:#}"))
    }
}
