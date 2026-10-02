// SPDX-License-Identifier: Apache-2.0
//! The client core's one error type.
//!
//! It is `Clone`, so that one failed fetch can be handed to every reader that was waiting for it.

use std::sync::Arc;

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    /// The server or the network failed; the SDK has already retried what it may.
    #[error(transparent)]
    Fetch(Arc<voidfs_sdk::Error>),
    /// The server answered with other content than the version asked for.
    #[error("{key} changed: asked for {expected}, got {got}")]
    Changed { key: String, expected: String, got: String },
    /// Fewer or more bytes than the range asked for.
    #[error("{key}: asked for {expected} bytes at {offset}, got {got}")]
    ShortRead { key: String, offset: u64, expected: u64, got: u64 },
    #[error("{0}")]
    Io(Arc<std::io::Error>),
    #[error("state database: {0}")]
    Db(Arc<rusqlite::Error>),
    /// Another process has the state directory.
    #[error("{0} is in use by another process")]
    Locked(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<voidfs_sdk::Error> for Error {
    fn from(e: voidfs_sdk::Error) -> Error {
        Error::Fetch(Arc::new(e))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(Arc::new(e))
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        Error::Db(Arc::new(e))
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(e: tokio::task::JoinError) -> Error {
        Error::Io(Arc::new(std::io::Error::other(e)))
    }
}
