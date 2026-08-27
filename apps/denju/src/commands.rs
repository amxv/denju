use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "denju",
    about = "Agent-native Agent Skills registry and synchronization",
    disable_version_flag = true,
    color = clap::ColorChoice::Never
)]
pub(crate) struct Cli {
    /// Emit one versioned JSON result on stdout.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub(crate) json: bool,
    /// Print the Denju build version.
    #[arg(short = 'V', long, action = ArgAction::SetTrue)]
    pub(crate) version: bool,
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Set up Denju on this machine without creating an account.
    Setup {
        /// Registry URL to use instead of Denju's default registry.
        #[arg(long, value_name = "URL")]
        registry: Option<String>,
    },
    /// Claim a new Denju username for this installation.
    Claim {
        /// Username to claim, with or without a leading @.
        #[arg(value_name = "USERNAME")]
        username: String,
    },
    /// Log this installation into an existing Denju identity.
    Login {
        /// Existing username, with or without a leading @.
        #[arg(value_name = "USERNAME")]
        username: String,
    },
    /// Show or manage identity and profile state.
    Identity {
        #[command(subcommand)]
        command: Option<IdentityCommand>,
    },
    /// List or revoke authenticated devices.
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    /// List, create, or revoke scoped automation tokens.
    Tokens {
        #[command(subcommand)]
        command: Option<TokenCommand>,
    },
    /// Search skills and packs visible to you.
    Search {
        /// Search terms.
        #[arg(value_name = "QUERY")]
        query: String,
        /// Sort matching public results.
        #[arg(long, value_enum, default_value_t = SearchSortArg::Relevance)]
        sort: SearchSortArg,
        /// Restrict discovery results to users you follow.
        #[arg(long, action = ArgAction::SetTrue)]
        following: bool,
        /// Restrict results to one topic.
        #[arg(long)]
        topic: Option<String>,
        /// Maximum number of results to return.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Continue a previous search page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show the highest-starred public skills.
    Top {
        /// Restrict the ranking to one topic.
        #[arg(long)]
        topic: Option<String>,
        /// Maximum number of results to return.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Continue a previous ranking page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show a user profile, skill, or pack.
    Show {
        /// @user, @owner/skill, or @owner/pack locator.
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Continue the followers list when showing a profile.
        #[arg(long)]
        followers_cursor: Option<String>,
        /// Continue the following list when showing a profile.
        #[arg(long)]
        following_cursor: Option<String>,
    },
    /// Follow a user for discovery.
    Follow {
        #[arg(value_name = "USERNAME")]
        username: String,
    },
    /// Stop following a user.
    Unfollow {
        #[arg(value_name = "USERNAME")]
        username: String,
    },
    /// Star a public skill.
    Star {
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Remove your star from a skill.
    Unstar {
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Replace the discovery topics on an owned resource.
    Topics {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Topic names. Pass none to clear the topic list.
        #[arg(value_name = "TOPIC")]
        topics: Vec<String>,
    },
    /// Privately report a public resource to registry operators.
    Report {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Short reason for the report.
        #[arg(long)]
        reason: String,
    },
    /// Import an unmanaged local skill into your private Denju workspace.
    Import {
        /// Path to a skill directory containing SKILL.md.
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Import into a team namespace instead of your personal namespace.
        #[arg(long, value_name = "@TEAM")]
        to: Option<String>,
    },
    /// Publish a skill release or make an owned pack public.
    Publish {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Make the resource publicly discoverable.
        #[arg(long, action = ArgAction::SetTrue)]
        public: bool,
        /// Release message for a skill publish.
        #[arg(long)]
        message: Option<String>,
        /// Add a release tag. May be repeated.
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Rename an owned skill or pack while preserving its stable resource ID.
    #[command(
        after_long_help = "Examples:\n  denju rename @amxv/new-noop noop\n  denju rename @acme/tooling platform-tooling"
    )]
    Rename {
        /// Existing owned skill or pack locator.
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// New unqualified resource name.
        #[arg(value_name = "NEW-NAME")]
        new_name: String,
    },
    /// Remove public visibility from an owned skill or pack.
    Unpublish {
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Tombstone an owned skill or pack.
    Delete {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Skip the interactive confirmation.
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
    /// Mark or unmark a released skill as deprecated.
    Deprecate {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Replacement skill to recommend to users.
        #[arg(long)]
        replacement: Option<String>,
        /// Remove the current deprecation marker.
        #[arg(long, action = ArgAction::SetTrue)]
        undo: bool,
    },
    /// Show namespace storage usage and queued local bytes.
    Usage,
    /// List every skill tracked by Denju on this machine.
    List,
    /// Show private-save and immutable release history, or prune old private history.
    History {
        /// Skill locator. Omit when using a history subcommand.
        #[arg(value_name = "LOCATOR")]
        locator: Option<String>,
        #[command(subcommand)]
        command: Option<HistoryCommand>,
    },
    /// Compare two skill revisions.
    Diff {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Older revision. Omit both revisions to compare recent local/remote state.
        #[arg(value_name = "REVISION-A")]
        revision_a: Option<String>,
        /// Newer revision.
        #[arg(value_name = "REVISION-B")]
        revision_b: Option<String>,
    },
    /// Restore an older revision as a new private revision.
    Restore {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        #[arg(value_name = "REVISION")]
        revision: String,
    },
    /// Export an accessible revision as an unmanaged directory.
    Export {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        #[arg(value_name = "DESTINATION")]
        destination: PathBuf,
    },
    /// Subscribe to a skill or live pack desired-state source.
    Subscribe {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Pin a skill subscription to one immutable release version.
        #[arg(long = "version", value_name = "N")]
        release_version: Option<u64>,
        /// Keep local bytes if the upstream resource is deleted.
        #[arg(long, action = ArgAction::SetTrue)]
        retain_on_delete: bool,
    },
    /// Remove a direct skill or pack subscription.
    Unsubscribe {
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Grant another user private read/subscription access to a skill.
    Share {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        #[arg(value_name = "@RECIPIENT")]
        recipient: String,
    },
    /// Remove another user's private access to a skill.
    Unshare {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        #[arg(value_name = "@RECIPIENT")]
        recipient: String,
    },
    /// Fork a skill, sync a fork, or resolve a local fork-name collision.
    Fork {
        /// Source skill to fork. Omit when using a fork subcommand.
        #[arg(value_name = "LOCATOR")]
        locator: Option<String>,
        #[command(subcommand)]
        command: Option<ForkCommand>,
    },
    /// Open a private proposal from a fork to its upstream maintainer.
    Propose {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Optional note for the upstream maintainer.
        #[arg(long)]
        message: Option<String>,
    },
    /// List proposals visible to you.
    Proposals,
    /// Inspect or act on one proposal.
    Proposal {
        #[command(subcommand)]
        command: ProposalCommand,
    },
    /// Create packs or change the skills in an owned pack.
    Pack {
        #[command(subcommand)]
        command: PackCommand,
    },
    /// Create, join, inspect, and manage teams.
    Team {
        #[command(subcommand)]
        command: Option<TeamCommand>,
    },
    /// Transfer a personal skill or pack into a team namespace.
    Transfer {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        #[arg(value_name = "@TEAM")]
        team: String,
    },
    /// Show local synchronization, conflict, and pending-work state.
    Status,
    /// Reconcile remote desired state, local workspaces, and harness projections now.
    Sync,
    /// Check and repair the local Denju installation.
    Doctor,
    /// Upgrade Denju with verification and rollback protection.
    Upgrade,
    /// Run the local synchronization daemon.
    #[command(hide = true)]
    Daemon,
    /// Internal upgrade health probe.
    #[command(hide = true)]
    UpgradeHealth,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IdentityCommand {
    /// Update public profile fields and social-list visibility.
    Update {
        /// Replace the profile bio.
        #[arg(long)]
        bio: Option<String>,
        /// Remove the current profile bio.
        #[arg(long, action = ArgAction::SetTrue, conflicts_with = "bio")]
        clear_bio: bool,
        /// Whether other users may see your followers list.
        #[arg(long, value_name = "BOOL")]
        followers_visible: Option<bool>,
        /// Whether other users may see who you follow.
        #[arg(long, value_name = "BOOL")]
        following_visible: Option<bool>,
    },
    /// Create an identity recovery backup.
    Backup,
    /// Recover an existing identity onto this installation.
    Recover {
        #[arg(value_name = "USERNAME")]
        username: String,
    },
    /// Permanently delete your Denju identity and owned resources.
    Delete {
        /// Skip the interactive confirmation.
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
    /// Delete old unreleased private revisions while preserving required history.
    Prune {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Skip the interactive confirmation.
        #[arg(long, action = ArgAction::SetTrue)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ForkCommand {
    /// Pull eligible upstream changes into an owned fork.
    Sync {
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Resolve a fork-name collision before promotion or projection.
    Resolve {
        #[arg(value_name = "LOCATOR")]
        locator: String,
        /// Rename the fork to this local resource name.
        #[arg(long = "as", value_name = "NAME")]
        as_name: Option<String>,
        /// Merge the fork into another owned resource.
        #[arg(long = "merge-into", value_name = "LOCATOR")]
        merge_into: Option<String>,
        /// Discard the colliding fork.
        #[arg(long, action = ArgAction::SetTrue)]
        discard: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProposalCommand {
    /// Show one proposal and its current state.
    Show {
        #[arg(value_name = "PROPOSAL-ID")]
        proposal_id: String,
    },
    /// Accept a proposal into the upstream skill.
    Accept {
        #[arg(value_name = "PROPOSAL-ID")]
        proposal_id: String,
    },
    /// Reject a proposal without changing the upstream skill.
    Reject {
        #[arg(value_name = "PROPOSAL-ID")]
        proposal_id: String,
    },
    /// Withdraw a proposal you previously opened.
    Withdraw {
        #[arg(value_name = "PROPOSAL-ID")]
        proposal_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PackCommand {
    /// Create an empty private pack.
    Create {
        /// New pack locator, for example @amxv/rust-tools.
        #[arg(value_name = "LOCATOR")]
        locator: String,
    },
    /// Add one or more skills to a pack atomically.
    Add {
        #[arg(value_name = "PACK")]
        locator: String,
        /// Skill locators to add.
        #[arg(required = true)]
        skills: Vec<String>,
    },
    /// Remove one or more skills from a pack atomically.
    Remove {
        #[arg(value_name = "PACK")]
        locator: String,
        /// Skill locators to remove.
        #[arg(required = true)]
        skills: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TeamCommand {
    /// Create a new team namespace.
    Create {
        #[arg(value_name = "@TEAM")]
        team: String,
    },
    /// Create a team invitation.
    Invite {
        #[arg(value_name = "@TEAM")]
        team: String,
        /// Role granted after the invite is accepted.
        #[arg(long, value_enum, default_value_t = TeamRoleArg::Member)]
        role: TeamRoleArg,
    },
    /// Revoke an unused team invitation.
    InviteRevoke {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "INVITE-ID")]
        invite_id: String,
    },
    /// Join a team with an invitation code.
    Join {
        #[arg(value_name = "CODE")]
        code: String,
    },
    /// Show team membership, settings, and assigned packs.
    Show {
        #[arg(value_name = "@TEAM")]
        team: String,
    },
    /// Change a member's team role.
    Role {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "@MEMBER")]
        member: String,
        #[arg(value_enum)]
        role: TeamRoleArg,
    },
    /// Remove a member from a team.
    Remove {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "@MEMBER")]
        member: String,
    },
    /// Change team publishing policy.
    Settings {
        #[arg(value_name = "@TEAM")]
        team: String,
        /// Allow ordinary team members to publish team-owned resources.
        #[arg(long, action = ArgAction::Set)]
        members_can_publish: bool,
    },
    /// Assign a pack as desired state for all team members.
    Assign {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "PACK")]
        pack: String,
    },
    /// Remove a team's assigned-pack relationship.
    Unassign {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "PACK")]
        pack: String,
    },
    /// Leave a team you do not own.
    Leave {
        #[arg(value_name = "@TEAM")]
        team: String,
    },
    /// Start transferring team ownership to another member.
    TransferOwner {
        #[arg(value_name = "@TEAM")]
        team: String,
        #[arg(value_name = "@MEMBER")]
        member: String,
    },
    /// Accept a pending team-ownership transfer.
    AcceptOwner {
        #[arg(value_name = "CODE")]
        code: String,
    },
    /// Permanently delete a team and its owned resources.
    Delete {
        #[arg(value_name = "@TEAM")]
        team: String,
        /// Skip the interactive confirmation.
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
    /// Revoke one authenticated device session.
    Revoke {
        #[arg(value_name = "SESSION-ID")]
        session_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TokenCommand {
    /// Create a scoped automation token.
    Create {
        /// Permission scope. May be repeated.
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,
        /// Token lifetime in seconds.
        #[arg(long, default_value_t = 86_400)]
        expires_in_seconds: u64,
    },
    /// Revoke an automation token.
    Revoke {
        #[arg(value_name = "TOKEN-ID")]
        token_id: String,
    },
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, error::ErrorKind};

    use super::*;

    #[test]
    fn every_command_has_descriptive_help() {
        fn visit(command: &clap::Command, path: &str) {
            for subcommand in command.get_subcommands() {
                let subpath = format!("{path} {}", subcommand.get_name());
                assert!(
                    subcommand.get_about().is_some(),
                    "{subpath} is missing command help"
                );
                visit(subcommand, &subpath);
            }
        }

        visit(&Cli::command(), "denju");
    }

    #[test]
    fn help_short_circuits_required_arguments() {
        for args in [
            vec!["denju", "rename", "--help"],
            vec!["denju", "rename", "-h"],
            vec!["denju", "team", "role", "--help"],
            vec!["denju", "tokens", "revoke", "--help"],
        ] {
            let error = Cli::try_parse_from(&args).expect_err("help exits through clap");
            assert_eq!(error.kind(), ErrorKind::DisplayHelp, "args: {args:?}");
            let rendered = error.to_string();
            assert!(rendered.contains("Usage:"), "args: {args:?}\n{rendered}");
            assert!(!rendered.contains("required arguments were not provided"));
        }
    }

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
