use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "denju",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    color = clap::ColorChoice::Never
)]
pub(crate) struct Cli {
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(crate) json: bool,
    #[arg(short = 'V', long, action = ArgAction::SetTrue)]
    pub(crate) version: bool,
    #[arg(short = 'h', long, global = true, action = ArgAction::SetTrue)]
    pub(crate) help: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Setup {
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    Claim {
        username: String,
    },
    Login {
        username: String,
    },
    Identity {
        #[command(subcommand)]
        command: Option<IdentityCommand>,
    },
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    Tokens {
        #[command(subcommand)]
        command: Option<TokenCommand>,
    },
    Search {
        query: String,
    },
    Show {
        locator: String,
    },
    Import {
        path: PathBuf,
    },
    Publish {
        locator: String,
        #[arg(long)]
        message: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    Rename {
        locator: String,
        new_name: String,
    },
    Unpublish {
        locator: String,
    },
    Delete {
        locator: String,
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
    Deprecate {
        locator: String,
        #[arg(long)]
        replacement: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        undo: bool,
    },
    Usage,
    History {
        locator: Option<String>,
        #[command(subcommand)]
        command: Option<HistoryCommand>,
    },
    Diff {
        locator: String,
        revision_a: Option<String>,
        revision_b: Option<String>,
    },
    Restore {
        locator: String,
        revision: String,
    },
    Export {
        locator: String,
        destination: PathBuf,
    },
    Subscribe {
        locator: String,
        #[arg(long = "version", value_name = "N")]
        release_version: Option<u64>,
        #[arg(long, action = ArgAction::SetTrue)]
        retain_on_delete: bool,
    },
    Unsubscribe {
        locator: String,
    },
    Share {
        locator: String,
        recipient: String,
    },
    Unshare {
        locator: String,
        recipient: String,
    },
    Fork {
        locator: Option<String>,
        #[command(subcommand)]
        command: Option<ForkCommand>,
    },
    Propose {
        locator: String,
        #[arg(long)]
        message: Option<String>,
    },
    Proposals,
    Proposal {
        #[command(subcommand)]
        command: ProposalCommand,
    },
    Status,
    Sync,
    Doctor,
    #[command(hide = true)]
    Daemon,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IdentityCommand {
    Backup,
    Recover {
        username: String,
    },
    Delete {
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum HistoryCommand {
    Prune {
        locator: String,
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ForkCommand {
    Sync {
        locator: String,
    },
    Resolve {
        locator: String,
        #[arg(long = "as", value_name = "NAME")]
        as_name: Option<String>,
        #[arg(long = "merge-into", value_name = "LOCATOR")]
        merge_into: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        discard: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    Show { proposal_id: String },
    Accept { proposal_id: String },
    Reject { proposal_id: String },
    Withdraw { proposal_id: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DevicesCommand {
    Revoke { session_id: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TokenCommand {
    Create {
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,
        #[arg(long, default_value_t = 86_400)]
        expires_in_seconds: u64,
    },
    Revoke {
        token_id: String,
    },
}
