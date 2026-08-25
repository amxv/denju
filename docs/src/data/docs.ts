export const siteConfig = {
  name: "denju",
  strapline: "Agent Skills, synchronized",
  description:
    "Denju is a registry and synchronization layer for Agent Skills: discover public skills, keep private skills synced across devices, share privately, manage team skill sets, or run the registry yourself.",
  repoUrl: "https://github.com/amxv/denju",
  accentColor: "#b4482b",
  accentColorDark: "#f08a64",
  footerSections: [
    {
      title: "denju",
      text: "A registry and synchronization system for Agent Skills, including private multi-device sync, private sharing, team distribution, and public discovery."
    },
    {
      title: "Documentation",
      text: "Task-first guides for using Denju, a separate self-hosting path, and optional architecture material for readers who want the internals."
    },
    {
      title: "Repository",
      linkPrefix: "Source: ",
      linkHref: "https://github.com/amxv/denju",
      linkLabel: "github.com/amxv/denju"
    }
  ]
} as const;

export const docCategories = [
  "Start",
  "Use Denju",
  "Self-host",
  "Architecture",
  "Reference",
  "Contributing"
] as const;

export const docCategoryDetails = {
  Start: "Install Denju, understand the mental model, and get your first subscribed skill working.",
  "Use Denju": "The everyday workflows: discover, publish, synchronize, collaborate, build packs, and run teams.",
  "Self-host": "Run an organization-owned registry with the same Denju server, PostgreSQL, and S3-compatible storage.",
  Architecture: "Optional internals: Merkle content, desired-state reconciliation, local projections, registry authority, and performance.",
  Reference: "CLI, automation output, troubleshooting, and exact behavioral contracts worth keeping nearby.",
  Contributing: "Repository orientation and safe development workflows for people and coding agents changing Denju itself."
} as const;

export const primaryNav = [
  { href: "/docs", label: "Docs" },
  { href: "/docs/self-host/quickstart", label: "Self-host" },
  { href: siteConfig.repoUrl, label: "GitHub", external: true }
];
