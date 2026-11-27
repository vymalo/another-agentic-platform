import type { Theme } from "vitepress";
import Md3Theme from "@vaam-apps/vitepress-md3";
import { Layout } from "./mermaid";
import "virtual:group-icons.css";
import "./extras.css";

// The pieces STRUCTURE.md v3 shares between the sites: the Mermaid renderer's layout, the group icons' stylesheet and the
// wider sidebar with the `::: in-depth` block (extras.css). The theme itself is `@vaam-apps/vitepress-md3`.
const theme: Theme = {
  ...Md3Theme,
  Layout,
};

export default theme;
