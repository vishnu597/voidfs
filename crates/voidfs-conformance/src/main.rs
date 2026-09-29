// SPDX-License-Identifier: Apache-2.0
//! `voidfs-conformance`: runs the conformance suite against an endpoint.

use std::collections::HashMap;
use std::process::ExitCode;

use clap::Parser;
use voidfs_conformance::runner::{Key, Outcome, Runner};
use voidfs_conformance::{CASES_JSON, cases};

/// Run the voidfs conformance suite against a server.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The server's endpoint, for example http://127.0.0.1:9000
    #[arg(long, env = "VOIDFS_ENDPOINT")]
    endpoint: Option<String>,
    /// Access key id of an admin-scoped key.
    #[arg(long, env = "VOIDFS_ACCESS_KEY_ID")]
    access_key_id: Option<String>,
    /// Secret of the admin-scoped key.
    #[arg(long, env = "VOIDFS_SECRET_ACCESS_KEY", hide_env_values = true)]
    secret_access_key: Option<String>,
    /// Access key id of a read-scoped key (enables cases that require key:read).
    #[arg(long, env = "VOIDFS_READ_ACCESS_KEY_ID")]
    read_access_key_id: Option<String>,
    /// Secret of the read-scoped key.
    #[arg(long, env = "VOIDFS_READ_SECRET_ACCESS_KEY", hide_env_values = true)]
    read_secret_access_key: Option<String>,
    /// Run only cases whose id contains this text (repeatable).
    #[arg(long = "case")]
    cases: Vec<String>,
    /// Use this case file instead of the bundled one.
    #[arg(long)]
    cases_file: Option<std::path::PathBuf>,
    /// List the cases and exit.
    #[arg(long)]
    list: bool,
    /// Print every request and response.
    #[arg(long, short)]
    verbose: bool,
    /// Address drives as `<drive>.<domain>` (virtual-host style) instead of by path, still
    /// connecting to --endpoint. The server must serve the domain (`--virtual-host-domain`).
    #[arg(long, env = "VOIDFS_VIRTUAL_HOST", value_name = "DOMAIN")]
    virtual_host: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let args = Args::parse();
    let json = match &args.cases_file {
        Some(p) => std::fs::read_to_string(p)?,
        None => CASES_JSON.to_owned(),
    };
    let suite = cases::load(&json)?;
    let selected: Vec<_> = suite
        .cases
        .iter()
        .filter(|c| args.cases.is_empty() || args.cases.iter().any(|f| c.id.contains(f.as_str())))
        .collect();
    if args.list {
        for c in &selected {
            println!("{:32} {}", c.id, c.title);
        }
        return Ok(ExitCode::SUCCESS);
    }

    let need = |v: Option<String>, what: &str| v.ok_or_else(|| anyhow::anyhow!("{what} is required (flag or environment)"));
    let mut keys = HashMap::new();
    keys.insert(
        "admin".to_string(),
        Key { id: need(args.access_key_id, "--access-key-id")?, secret: need(args.secret_access_key, "--secret-access-key")? },
    );
    if let (Some(id), Some(secret)) = (args.read_access_key_id, args.read_secret_access_key) {
        keys.insert("read".into(), Key { id, secret });
    }
    let mut runner = Runner::new(&need(args.endpoint, "--endpoint")?, keys)?;
    runner.verbose = args.verbose;
    runner.virtual_host = args.virtual_host;

    let (mut pass, mut fail, mut skip) = (0, 0, 0);
    for case in selected {
        if runner.verbose {
            eprintln!("{}", case.id);
        }
        let r = runner.run_case(case).await;
        match &r.outcome {
            Outcome::Pass => {
                pass += 1;
                println!("PASS  {:32} {:>6} ms", r.id, r.elapsed.as_millis());
            }
            Outcome::Fail { step, message } => {
                fail += 1;
                println!("FAIL  {:32} step {step}: {message}", r.id);
            }
            Outcome::Skip(why) => {
                skip += 1;
                println!("SKIP  {:32} {why}", r.id);
            }
        }
    }
    println!("\n{pass} passed, {fail} failed, {skip} skipped");
    Ok(if fail == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
