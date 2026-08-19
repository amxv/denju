export const siteConfig = {
  name: "agentbox-skill-share",
  strapline: "Package and share complete agent skills",
  description:
    "Documentation for packaging complete agent skill directories and sharing them with a team through Agentbox.",
  repoUrl: "https://github.com/amxv/agentbox-skill-share",
  accentColor: "#0369a1",
  accentColorDark: "#38bdf8",
  footerSections: [
    {
      title: "agentbox-skill-share",
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
      linkHref: "https://github.com/amxv/agentbox-skill-share",
      linkLabel: "github.com/amxv/agentbox-skill-share"
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
