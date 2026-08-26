use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

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
        #[arg(long, value_enum, default_value_t = SearchSortArg::Relevance)]
        sort: SearchSortArg,
        #[arg(long, action = ArgAction::SetTrue)]
        following: bool,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        cursor: Option<String>,
    },
    Top {
        #[arg(long)]
        topic: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        cursor: Option<String>,
    },
    Show {
        locator: String,
        #[arg(long)]
        followers_cursor: Option<String>,
        #[arg(long)]
        following_cursor: Option<String>,
    },
    Follow {
        username: String,
    },
    Unfollow {
        username: String,
    },
    Star {
        locator: String,
    },
    Unstar {
        locator: String,
    },
    Topics {
        locator: String,
        #[arg(value_name = "TOPIC")]
        topics: Vec<String>,
    },
    Report {
        locator: String,
        #[arg(long)]
        reason: String,
    },
    Import {
        path: PathBuf,
        #[arg(long, value_name = "@TEAM")]
        to: Option<String>,
    },
    Publish {
        locator: String,
        #[arg(long, action = ArgAction::SetTrue)]
        public: bool,
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
    List,
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
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },
    Team {
        #[command(subcommand)]
        command: Option<TeamCommand>,
    },
    Transfer {
        locator: String,
        team: String,
    },
    Status,
    Sync,
    Doctor,
    Upgrade,
    #[command(hide = true)]
    Daemon,
    #[command(hide = true)]
    UpgradeHealth,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IdentityCommand {
    Update {
        #[arg(long)]
        bio: Option<String>,
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "bio")]
        clear_bio: bool,
        #[arg(long, value_name = "BOOL")]
        followers_visible: Option<bool>,
        #[arg(long, value_name = "BOOL")]
        following_visible: Option<bool>,
    },
    Backup,
    Recover {
        username: String,
    },
    Delete {
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SearchSortArg {
    Relevance,
    Stars,
}

impl SearchSortArg {
    pub(crate) const fn to_wire(self) -> denju_wire::SearchSort {
        match self {
            Self::Relevance => denju_wire::SearchSort::Relevance,
            Self::Stars => denju_wire::SearchSort::Stars,
        }
    }
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
pub(crate) enum PackCommand {
    Create {
        locator: String,
    },
    Add {
        locator: String,
        #[arg(required = true)]
        skills: Vec<String>,
    },
    Remove {
        locator: String,
        #[arg(required = true)]
        skills: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TeamCommand {
    Create {
        team: String,
    },
    Invite {
        team: String,
        #[arg(long, value_enum, default_value_t = TeamRoleArg::Member)]
        role: TeamRoleArg,
    },
    InviteRevoke {
        team: String,
        invite_id: String,
    },
    Join {
        code: String,
    },
    Show {
        team: String,
    },
    Role {
        team: String,
        member: String,
        #[arg(value_enum)]
        role: TeamRoleArg,
    },
    Remove {
        team: String,
        member: String,
    },
    Settings {
        team: String,
        #[arg(long, action = ArgAction::Set)]
        members_can_publish: bool,
    },
    Assign {
        team: String,
        pack: String,
    },
    Unassign {
        team: String,
        pack: String,
    },
    Leave {
        team: String,
    },
    TransferOwner {
        team: String,
        member: String,
    },
    AcceptOwner {
        code: String,
    },
    Delete {
        team: String,
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TeamRoleArg {
    Member,
    Maintainer,
}

impl TeamRoleArg {
    pub(crate) const fn to_wire(self) -> denju_wire::TeamRole {
        match self {
            Self::Member => denju_wire::TeamRole::Member,
            Self::Maintainer => denju_wire::TeamRole::Maintainer,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_commands_expose_stable_filters_and_cursors() {
        let cli = Cli::try_parse_from([
            "denju",
            "search",
            "rust agents",
            "--sort",
            "stars",
            "--following",
            "--topic",
            "rust",
            "--limit",
            "7",
            "--cursor",
            "abc",
        ])
        .unwrap();
        let Some(Command::Search {
            sort,
            following,
            topic,
            limit,
            cursor,
            ..
        }) = cli.command
        else {
            panic!("expected search command");
        };
        assert_eq!(sort, SearchSortArg::Stars);
        assert!(following);
        assert_eq!(topic.as_deref(), Some("rust"));
        assert_eq!(limit, 7);
        assert_eq!(cursor.as_deref(), Some("abc"));

        let cli = Cli::try_parse_from([
            "denju",
            "show",
            "@alice",
            "--followers-cursor",
            "f",
            "--following-cursor",
            "g",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Show {
                followers_cursor: Some(ref followers),
                following_cursor: Some(ref following),
                ..
            }) if followers == "f" && following == "g"
        ));
    }

    #[test]
    fn profile_update_is_part_of_identity_without_a_second_profile_command() {
        let cli = Cli::try_parse_from([
            "denju",
            "identity",
            "update",
            "--bio",
            "agent builder",
            "--followers-visible",
            "false",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Identity {
                command: Some(IdentityCommand::Update {
                    bio: Some(ref bio),
                    followers_visible: Some(false),
                    ..
                }),
            }) if bio == "agent builder"
        ));
    }

    #[test]
    fn upgrade_is_a_top_level_maintenance_command() {
        let cli = Cli::try_parse_from(["denju", "upgrade"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Upgrade)));
    }

    #[test]
    fn list_is_a_top_level_inventory_command() {
        let cli = Cli::try_parse_from(["denju", "list"]).unwrap();
        assert!(matches!(cli.command, Some(Command::List)));
    }
}
