// SPDX-License-Identifier: Apache-2.0
//! `void`, the voidfs command line: drives, forks, history, show, restore and upload, on the
//! protocol through `voidfs-sdk`. With `--json`, every command prints one JSON document on stdout,
//! and errors as JSON on stderr, with a nonzero exit status.

mod config;
mod drives;
mod history;
mod keys;
mod output;
mod upload;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::output::{Failure, Out, Result};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("VOID_COMMIT"), ", ", env!("VOID_COMMIT_DATE"), ")");

#[derive(Parser, Debug)]
#[command(
    name = "void",
    version = VERSION,
    about = "void — voidfs drives from a terminal: create, fork and delete them, read and restore their history, upload into them",
    after_help = "Start here:\n  void keys generate --scope admin   make a key, then start voidfs-server with it\n  void drives                        see every drive the key reaches\n  void drive create <name>           make one\n  void upload <paths…> <drive>:/     upload into it\n\nThe server and key come from VOIDFS_ENDPOINT, VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY,\nor the flags below. Drives are named by alias or id everywhere."
)]
struct Cli {
    #[command(flatten)]
    connection: config::Connection,
    /// Print one JSON document on stdout, and errors as JSON on stderr
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Every drive the key reaches: its name, id, size, when it was made and what it forks
    #[command(visible_alias = "ls")]
    Drives,
    /// Create, show, delete or recover a drive
    #[command(subcommand)]
    Drive(DriveCommand),
    /// Fork a drive: a copy-on-write copy of its current state, ready at once whatever its size
    Fork {
        /// The drive to fork, by alias or id
        source: String,
        /// The new drive's name: 3–63 lowercase letters, digits, `-` and `.`
        name: String,
    },
    /// List the versions of a file, or of every file in a folder, and when each was made
    History {
        #[command(flatten)]
        target: history::Target,
        /// Include renames and changes of attributes, not only changes of content
        #[arg(long)]
        all: bool,
    },
    /// Print a file as it was at an earlier time, or as it is, without changing anything
    Show {
        #[command(flatten)]
        target: history::Target,
        /// The content the file had AT this time: RFC 3339 (`2026-06-15T12:00:00Z`) or Unix seconds
        #[arg(long, value_name = "TIME", conflicts_with = "version")]
        at: Option<String>,
        /// A version id, as `void history` lists them
        #[arg(long, value_name = "ID")]
        version: Option<String>,
        /// Write the file here instead of to stdout (`-` for stdout)
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Roll a file or folder back to an earlier time, as a new version: nothing is lost
    #[command(group(clap::ArgGroup::new("when").required(true).args(["at", "version"])))]
    Restore {
        #[command(flatten)]
        target: history::Target,
        /// Roll back to the state AT this time: RFC 3339 or Unix seconds. A folder rolls its
        /// whole subtree back, and needs it
        #[arg(long, value_name = "TIME")]
        at: Option<String>,
        /// Roll a file back to this version id, as `void history` lists them. A deleted file or
        /// folder comes back this way, from the last version it had
        #[arg(long, value_name = "ID")]
        version: Option<String>,
    },
    /// Upload local files or folders into a drive, in the foreground
    Upload(upload::UploadArgs),
    /// Print the version and the revision it was built from
    Version,
    /// Access keys for voidfs-server
    #[command(subcommand)]
    Keys(KeysCommand),
}

#[derive(Subcommand, Debug)]
enum DriveCommand {
    /// Create a drive
    Create {
        /// Its name, the alias it is reached by: 3–63 lowercase letters, digits, `-` and `.`
        name: String,
        /// A name for people (the server keeps the alias for now)
        #[arg(long, value_name = "NAME")]
        display_name: Option<String>,
    },
    /// A drive's id, size, lineage and position
    Show {
        /// The drive, by alias or id
        drive: String,
    },
    /// Delete a drive. It is recoverable for the retention window (30 days by default), then gone
    Delete {
        /// The drive, by alias or id
        drive: String,
        /// Delete it permanently, now: it can't be recovered. Content its forks share stays
        #[arg(long)]
        hard: bool,
        /// Do not ask for its name to confirm
        #[arg(short, long)]
        yes: bool,
    },
    /// Recover a deleted drive within the retention window
    Undelete {
        /// The drive's alias
        drive: String,
    },
}

#[derive(Subcommand, Debug)]
enum KeysCommand {
    /// Make a key for `voidfs-server --key`. The secret is printed once; the server keeps no copy
    /// until you give it one
    Generate {
        /// What the key may do
        #[arg(long, visible_alias = "access", value_enum, default_value_t = keys::Scope::Write)]
        scope: keys::Scope,
        /// How to print the key
        #[arg(long, value_enum, default_value_t = keys::Format::Text)]
        format: keys::Format,
    },
}

async fn run(cli: Cli) -> Result<()> {
    let out = Out { json: cli.json };
    let client = || cli.connection.client(|n| std::env::var(n).ok());
    match &cli.command {
        Command::Version => out.emit(
            &json!({ "name": "void", "version": env!("CARGO_PKG_VERSION"), "commit": env!("VOID_COMMIT"), "commitDate": env!("VOID_COMMIT_DATE"), "protocol": 1 }),
            || format!("void {VERSION}"),
        ),
        Command::Keys(KeysCommand::Generate { scope, format }) => keys::generate(out, *scope, *format),
        Command::Drives => drives::list(&client()?, out).await,
        Command::Drive(DriveCommand::Create { name, display_name }) => drives::create(&client()?, out, name, display_name.clone()).await,
        Command::Drive(DriveCommand::Show { drive }) => drives::show(&client()?, out, drive).await,
        Command::Drive(DriveCommand::Delete { drive, hard, yes }) => drives::delete(&client()?, out, drive, *hard, *yes).await,
        Command::Drive(DriveCommand::Undelete { drive }) => drives::undelete(&client()?, out, drive).await,
        Command::Fork { source, name } => drives::fork(&client()?, out, source, name).await,
        Command::History { target, all } => history::history(&client()?, out, target, *all).await,
        Command::Show { target, at, version, output } => history::show(&client()?, out, target, at.as_deref(), version.as_deref(), output.as_deref()).await,
        Command::Restore { target, at, version } => history::restore(&client()?, out, target, at.as_deref(), version.as_deref()).await,
        Command::Upload(args) => upload::upload(&client()?, out, args).await,
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind;
            let json = std::env::args_os().any(|a| a == "--json");
            if json && !matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) {
                let message = e.render().to_string();
                let first = message.lines().next().unwrap_or_default().trim_start_matches("error: ").to_owned();
                Failure::usage(first).print(true);
                return ExitCode::from(2);
            }
            e.exit();
        }
    };
    let json = cli.json;
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            Failure::new("InternalError", format!("starting the runtime: {e}")).print(json);
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            f.print(json);
            ExitCode::from(f.exit)
        }
    }
}
