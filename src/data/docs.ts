export const siteConfig = {
  name: "denju",
  strapline: "Package and share complete agent skills",
  description:
    "Documentation for packaging complete agent skill directories and sharing them with a team through Agentbox.",
  repoUrl: "https://github.com/amxv/denju",
  accentColor: "#0369a1",
  accentColorDark: "#38bdf8",
  footerSections: [
    {
      title: "denju",
      text:
        "A focused Go CLI for moving complete reusable skills between Agentbox teammates."
    },
    {
      title: "What this site covers",
      text:
        "Installation, authentication, bundle behavior, command options, and release maintenance."
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
  "Guides",
  "Distribution",
  "Reference"
] as const;

export const primaryNav = [
  { href: "/docs", label: "Docs" },
  { href: siteConfig.repoUrl, label: "GitHub", external: true }
];
