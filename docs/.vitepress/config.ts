import { readFileSync } from "node:fs";
import { dirname, join, posix } from "node:path";
import { fileURLToPath } from "node:url";
import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";
import { withMermaid } from "vitepress-plugin-mermaid";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// Sidebar entries for the listed pages, titled by each page's first heading.
function pages(dir: string, files: string[]) {
  return files.map((f) => {
    const text = readFileSync(join(root, dir, f), "utf8").match(/^# (.+)$/m)?.[1];
    return { text: (text ?? f.replace(/\.md$/, "")).replace(/`/g, ""), link: `/${dir}/${f.replace(/\.md$/, "")}` };
  });
}

// The shared section structure: see STRUCTURE.md. The system's and adam-rs's sites use the same one.
// Files are mapped, not moved: docs/architecture/, docs/extensions/ and docs/api/ stay where tools/docs-check and the
// OpenAPI drift test read them.
const sidebar = [
  {
    text: "Overview",
    items: [
      { text: "another-agentic-platform", link: "/" },
      { text: "MVP cut", link: "/mvp" },
    ],
  },
  { text: "Get started", items: [{ text: "Get started", link: "/get-started" }] },
  {
    text: "Guides",
    collapsed: true,
    items: [
      { text: "Guides", link: "/guides/" },
      { text: "Deploy", items: pages("guides/deploy", ["install.md", "platform-api.md", "run-pods.md", "metering.md"]) },
      { text: "Operate", items: pages("guides/operate", ["not-ready.md", "upgrade.md", "replicas.md", "metrics.md", "traces.md"]) },
    ],
  },
  {
    text: "Reference",
    collapsed: true,
    items: [
      { text: "Reference", link: "/reference/" },
      {
        text: "API",
        items: [
          { text: "The Platform API's contract", link: "/api/" },
          { text: "OpenAPI document", link: "/reference/openapi" },
          ...pages("api", ["usage-ingest-v1.md"]),
        ],
      },
      { text: "CRDs", items: [{ text: "Custom resources", link: "/reference/crds" }] },
      { text: "Extensions", items: [{ text: "Extensions", link: "/extensions/" }, ...pages("extensions", ["release-channels-v1.md", "agent-registry-v1.md"])] },
    ],
  },
  {
    text: "Architecture",
    collapsed: true,
    items: [
      { text: "Index", link: "/architecture/" },
      ...pages("architecture", [
        "01-overview.md",
        "02-domain-model.md",
        "03-interfaces.md",
        "04-workflows.md",
        "05-runtime.md",
        "06-environments-and-storage.md",
        "07-tools.md",
        "08-security.md",
        "09-operations.md",
        "10-control-plane-and-crds.md",
        "12-summary.md",
      ]),
    ],
  },
  { text: "Decisions", items: pages("architecture", ["11-decisions.md"]) },
  { text: "Contributing", items: [{ text: "How to contribute", link: "/contributing" }] },
];

// Prose writes <agent> or <ThreadShare> meaning a placeholder; Vue would read it as a component and fail the build.
// Only real HTML elements pass through, everything else is shown as the text it is.
const HTML = new Set(
  "a abbr blockquote br code details dd div dl dt em figcaption figure h1 h2 h3 h4 h5 h6 hr i img kbd li ol p picture pre small source span strong sub summary sup table tbody td tfoot th thead tr u ul video".split(" "),
);
export function escapeUnknownTags(md: any) {
  // `{{` is a Vue expression to the compiler; the docs mean the characters.
  const render = md.render.bind(md);
  md.render = (src: string, env: unknown) => render(src, env).replace(/\{\{/g, "&#123;&#123;");
  for (const rule of ["html_inline", "html_block"]) {
    const original = md.renderer.rules[rule];
    md.renderer.rules[rule] = (tokens: any, i: number, ...rest: any[]) => {
      const content: string = tokens[i].content;
      const tags = [...content.matchAll(/<\/?([A-Za-z][\w-]*)/g)].map((m) => m[1].toLowerCase());
      const wellFormed = /^\s*(<\/?[A-Za-z][\w-]*(\s+[\w:-]+(="[^"]*")?)*\s*\/?>\s*)+$/.test(content) || tokens[i].type === "html_block";
      if (wellFormed && tags.every((t) => HTML.has(t))) return original(tokens, i, ...rest);
      return md.utils.escapeHtml(content).replace(/\{\{/g, "&#123;&#123;");
    };
  }
}

// GitHub's slug: lowercase, drop everything but letters, digits, marks, space, `-` and `_`, each space becomes `-`.
export function githubSlug(s: string): string {
  return s.trim().toLowerCase().replace(/[^\p{L}\p{M}\p{N}_ -]/gu, "").replace(/ /g, "-");
}

export default withMermaid(
  defineMd3Config({
    title: "another-agentic-platform",
    description: "A Kubernetes-native platform for durable, versioned AI agent services on disposable compute.",
    base: process.env.DOCS_BASE ?? "/another-agentic-platform/",
    cleanUrls: true,
    // VitePress serves index.md; these READMEs are the folders' own index pages and stay where they are.
    rewrites: { "architecture/README.md": "architecture/index.md", "api/README.md": "api/index.md" },
    // Links that leave docs/ are rewritten to GitHub below, so every dead link left fails the build.
    srcExclude: ["**/node_modules/**"],
    themeConfig: {
      md3: { style: "expressive", seed: "#0f766e" },
      nav: [
        { text: "Overview", link: "/" },
        { text: "Get started", link: "/get-started" },
        { text: "Reference", link: "/reference/" },
        { text: "Architecture", link: "/architecture/" },
        { text: "Decisions", link: "/architecture/11-decisions" },
      ],
      sidebar,
      outline: { level: [2, 3] },
      search: { provider: "local" },
      editLink: { pattern: "https://github.com/vymalo/another-agentic-platform/edit/main/docs/:path" },
    },
    markdown: {
      // GitHub's heading ids, so a `#59a-...` or `#ad-053--...` link written for GitHub lands here too: VitePress would
      // prefix `_` to an id that starts with a digit and keep the em dash. Repeats become -1, -2 (the anchor plugin does that).
      anchor: { slugify: githubSlug },
      config: (md: any) => {
        escapeUnknownTags(md);
        // The rewrites above serve a folder's README.md as its index; a link written to the file (which tools/docs-check
        // resolves) must land on that index, not on a page that does not exist.
        md.core.ruler.push("readme-links", (state: any) => {
          const repo = "https://github.com/vymalo/another-agentic-platform";
          const from = posix.dirname(state.env?.relativePath ?? "index.md");
          for (const block of state.tokens)
            for (const t of block.children ?? [])
              if (t.type === "link_open") {
                const href = t.attrGet("href");
                if (!href || href.startsWith("#") || href.startsWith("/") || /^[a-z][a-z0-9+.-]*:/i.test(href)) continue;
                const [, path, rest = ""] = href.match(/^([^#?]*)(.*)$/)!;
                const resolved = posix.normalize(posix.join(from, decodeURI(path)));
                const isDir = path.endsWith("/") || posix.extname(resolved) === "";
                const outside = resolved === ".." || resolved.startsWith("../");
                // A link out of docs/, or to a file in docs/ that is not a page (json, yaml, images): the file on GitHub.
                if (outside || (posix.extname(resolved) !== "" && posix.extname(resolved) !== ".md")) {
                  const repoPath = outside ? posix.normalize(resolved.replace(/^(\.\.\/)/, "")) : `docs/${resolved}`;
                  t.attrSet("href", `${repo}/${isDir ? "tree" : "blob"}/main/${repoPath.replace(/\/$/, "")}${rest}`);
                  continue;
                }
                t.attrSet("href", href.replace(/(^|\/)README\.md(?=#|$)/, "$1index.md"));
              }
        });
      },
    },
  }),
);
