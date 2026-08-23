pub const HELP: &str = "Denju — agent-native Agent Skills registry and synchronization\n\
\n\
Usage: denju [OPTIONS] [COMMAND]\n\
\n\
Commands:\n\
  setup   Set up this machine without creating an account\n\
  claim   Claim a Denju identity for this installation\n\
  login   Log this installation into an existing identity\n\
  identity  Show or update profile/identity state, recover, back up, or delete it\n\
  devices List or revoke authenticated devices\n\
  tokens  List, create, or revoke scoped automation credentials\n\
  search  Search the merged local, private, team, and public catalog\n\
  top     Rank public skills by all-time stars\n\
  show    Show a profile, accessible skill, or pack; profile lists support cursor continuation\n\
  follow  Follow a user for discovery\n\
  unfollow  Stop following a user\n\
  star    Star a public skill\n\
  unstar  Remove your star from a skill\n\
  topics  Set discovery topics for an owned resource\n\
  report  Privately report a public resource to registry operators\n\
  import  Transfer a local skill into your private Denju workspace\n\
  publish Publish a skill release or make an owned pack public\n\
  rename  Rename an owned skill or pack while preserving its stable resource ID\n\
  unpublish  Remove public visibility from an owned skill or pack\n\
  delete  Tombstone an owned skill or pack\n\
  deprecate  Mark or unmark a released skill as deprecated\n\
  usage   Show namespace storage usage and queued local bytes\n\
  history Show private-save and immutable release history\n\
  diff    Compare two revisions\n\
  restore Restore an older revision as a new private revision\n\
  export  Export an accessible revision as an unmanaged directory\n\
  subscribe   Subscribe to a skill or live pack desired-state source\n\
  unsubscribe Remove a direct skill or pack subscription\n\
  share   Grant another user private read/subscription access\n\
  unshare Remove a user's private read/subscription access\n\
  fork    Fork a skill, sync a fork from upstream, or resolve a claim collision\n\
  propose Open a private proposal from a fork to its upstream maintainer\n\
  proposals  List proposals visible to you\n\
  proposal   Show, accept, reject, or withdraw a proposal\n\
  pack    Create a pack or atomically add/remove skill members\n\
  team    Create, join, inspect, and manage teams\n\
  transfer  Transfer a personal skill or pack into a team\n\
  status  Show local synchronization and conflict state\n\
  sync    Reconcile subscriptions and harness projections\n\
  doctor  Check and repair the local Denju installation\n\
\n\
Options:\n\
      --json     Emit one versioned JSON result on stdout\n\
  -V, --version  Print the Denju build version\n\
  -h, --help     Print help\n\
\n\
Run denju with no command for the next useful action.";
