export const siteConfig = {
  name: "denju",
  strapline: "Agent Skills, synchronized",
  description: "Documentation for Denju, a native Rust CLI and registry for managing Agent Skills across agent harnesses.",
  repoUrl: "https://github.com/amxv/denju",
  accentColor: "#0369a1",
  accentColorDark: "#38bdf8",
  footerSections: [
    { title: "denju", text: "A native Rust CLI and registry for managing Agent Skills across agent harnesses." },
    { title: "Docs", text: "Human-readable documentation with raw Markdown routes for agents and tools." },
    { title: "Repository", linkPrefix: "Source: ", linkHref: "https://github.com/amxv/denju", linkLabel: "github.com/amxv/denju" }
  ]
} as const;

export const docCategories = ["Start", "Development", "Reference"] as const;
export const primaryNav = [
  { href: "/docs", label: "Docs" },
  { href: siteConfig.repoUrl, label: "GitHub", external: true }
];
