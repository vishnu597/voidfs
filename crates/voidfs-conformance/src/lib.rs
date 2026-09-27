// SPDX-License-Identifier: Apache-2.0
//! Runs the voidfs conformance suite (`spec/conformance/cases.json`) against a live endpoint.
//! The case format is defined in `spec/conformance/README.md`.

pub mod cases;
pub mod check;
pub mod runner;

/// The suite, compiled in so the runner works from any directory.
pub const CASES_JSON: &str = include_str!("../../../spec/conformance/cases.json");
