import { defineConfig } from "astro/config";
import zuedocs from "zuedocs/astro";

export default defineConfig({
  output: "static",
  site: "https://github.com/amxv/agentbox-skill-share",
  integrations: [zuedocs()]
});
