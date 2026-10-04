// SPDX-License-Identifier: Apache-2.0
//! `voidfs-server` as a library: what the binary is made of, and [`test_server`], a server in the
//! same process for the tests of clients.

pub mod admin;
pub mod clock;
pub mod direct;
pub mod gc;
pub mod metrics;
pub mod pool;
pub mod probe;
pub mod s3;
pub mod sigv4;
pub mod store;
pub mod test_server;
