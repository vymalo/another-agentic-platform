import type { Theme } from "vitepress";
import Md3Theme from "@vaam-apps/vitepress-md3";
import { theme as openapiTheme, useOpenapi } from "vitepress-openapi/client";
import "vitepress-openapi/dist/style.css";
import { Layout } from "./mermaid";
import "virtual:group-icons.css";
import "./extras.css";

// The pieces STRUCTURE.md v3 shares between the sites: the Mermaid renderer's layout, the group icons' stylesheet and the
// wider sidebar with the `::: in-depth` block (extras.css). The theme itself is `@vaam-apps/vitepress-md3`.
//
// The OpenAPI library's theme is merged here for its components (`<OASpec>`, `<OAOperation>`), and its own configuration
// is set once: no "Try it out", because the docs site is not a client of any deployment. The contract itself is loaded by
// the API pages (openapi/client.ts), never by the theme every page loads.
const theme: Theme = {
  ...Md3Theme,
  Layout,
  async enhanceApp(ctx) {
    await Md3Theme.enhanceApp?.(ctx);
    useOpenapi({
      config: { operation: { hiddenSlots: ["playground"] } },
    });
    await openapiTheme.enhanceApp?.(ctx);
  },
};

export default theme;
