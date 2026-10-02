// SPDX-License-Identifier: Apache-2.0
//! The revision `void version` reports: the commit built and its date, from git when there is one.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn main() {
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let date = git(&["log", "-1", "--format=%cs", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=VOID_COMMIT={commit}");
    println!("cargo:rustc-env=VOID_COMMIT_DATE={date}");
    // Built again when HEAD moves. A path that doesn't exist would rebuild every time.
    let mut watch = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watch.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for p in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &p]).filter(|p| std::path::Path::new(p).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
