export const siteConfig = {
  name: "denju",
  strapline: "Agent-native skills, synchronized",
  description: "Documentation for Denju, an agent-native social registry and synchronization system for Agent Skills.",
  repoUrl: "https://github.com/amxv/denju",
  accentColor: "#0369a1",
  accentColorDark: "#38bdf8",
  footerSections: [
    { title: "denju", text: "A native Rust CLI and registry for discovering, publishing, and synchronizing Agent Skills." },
    { title: "Status", text: "The Rust implementation is being built from the product specification." },
    { title: "Repository", linkPrefix: "Source: ", linkHref: "https://github.com/amxv/denju", linkLabel: "github.com/amxv/denju" }
  ]
} as const;

export const docCategories = ["Start", "Development", "Reference"] as const;
export const primaryNav = [
  { href: "/docs", label: "Docs" },
  { href: siteConfig.repoUrl, label: "GitHub", external: true }
];
