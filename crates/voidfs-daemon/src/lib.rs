// SPDX-License-Identifier: Apache-2.0
//! The voidfs per-user agent (step 4, item 4 of the parity plan), which `void daemon run` runs:
//! the client core of `voidfs-client` behind a Unix socket in the state directory, with HTTP and
//! JSON over it.
//!
//! - [`Daemon`]: owns the state directory (the store, the cache, the upload queue and the link to
//!   the server) and serves the socket.
//! - [`api`]: what the socket speaks, which the Mac app speaks too.
//! - [`DaemonClient`]: a client for the socket, which `void` uses.
//! - [`Settings`]: the connection `void daemon start` leaves for the daemon in `daemon.json`.
//! - [`Adapter`]: what mounts a drive (step 5); the daemon keeps the mount table.

pub mod api;
mod client;
mod mounts;
mod server;
mod settings;
mod uploads;

use std::path::{Path, PathBuf};

pub use client::{ClientError, DaemonClient, UploadWatch};
pub use mounts::{Adapter, Core, MountSpec, Mounted};
pub use server::{Daemon, DaemonConfig, Error};
pub use settings::Settings;

/// The socket's name in the state directory.
pub const SOCKET: &str = "daemon.sock";
/// The log `void daemon start` sends the daemon's output to, in the state directory.
pub const LOG: &str = "daemon.log";

/// Where the daemon of state directory `dir` listens.
pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join(SOCKET)
}

/// The longest path a Unix socket may have here: `sun_path` holds it with its terminating NUL.
pub const MAX_SOCKET_PATH: usize = if cfg!(any(target_os = "linux", target_os = "android")) { 107 } else { 103 };
