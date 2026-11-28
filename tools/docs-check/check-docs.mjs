// Validates the repository's Markdown:
//   1. every ```mermaid block parses with the pinned Mermaid version, and
//   2. every relative Markdown link points at a file or directory that exists.
// Usage (from the repo root):  npm --prefix tools/docs-check ci && node tools/docs-check/check-docs.mjs
// Exits 1 on any failure, listing file:line for each.
import fs from 'node:fs';
import path from 'node:path';
import { JSDOM } from 'jsdom';

const root = process.cwd();
const skipDirs = new Set(['.git', 'node_modules']);

function* markdownFiles(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (skipDirs.has(entry.name)) continue;
    const full = path.join(dir, entry.name);
    if (entry.isSymbolicLink()) continue; // symlinked skills/AGENTS.md are checked at their target
    if (entry.isDirectory()) yield* markdownFiles(full);
    else if (entry.name.endsWith('.md')) yield full;
  }
}

// Mermaid needs a DOM to parse.
const dom = new JSDOM('<!doctype html><html><body></body></html>');
globalThis.window = dom.window;
globalThis.document = dom.window.document;
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.DOMParser = dom.window.DOMParser;
globalThis.Element = dom.window.Element;
const { default: mermaid } = await import('mermaid');
mermaid.initialize({ startOnLoad: false });

const lineOf = (src, index) => src.slice(0, index).split('\n').length;

// `::: name` opens a container and a bare `:::` closes the innermost one; a fenced block is code (the same character, at
// least as long, nothing after it closes it). Returns the line of each `::: in-depth` left open (STRUCTURE.md v3).
function unclosedInDepth(src) {
  const open = [];
  let fence = null;
  const lines = src.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trimStart();
    const f = /^(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (f && f[1][0] === fence.ch && f[1].length >= fence.len && f[2].trim() === '') fence = null;
      continue;
    }
    if (f && !(f[1][0] === '`' && f[2].includes('`'))) { fence = { ch: f[1][0], len: f[1].length }; continue; }
    const m = /^:{3,}\s*(.*?)\s*$/.exec(line);
    if (!m) continue;
    if (m[1] === '') open.pop();
    else open.push({ name: m[1].split(/\s/)[0], line: i + 1 });
  }
  return open.filter((o) => o.name === 'in-depth').map((o) => o.line);
}
let diagrams = 0, links = 0;
const failures = [];
// Every markdown file (relative path) to its relative link targets as absolute paths (for the pair rule).
const linksOut = new Map();

// The docs for people and the docs for agents live apart (AD-074): the dated records keep
// their evidence as written, and the pages of stillMixed are mixed and named until split.
const datedRecords = new Set(['docs/architecture/11-decisions.md', 'docs/architecture/README.md']);
const stillMixed = new Set(['docs/architecture/10-control-plane-and-crds.md']);
// Something like `crates/x/src/y.rs:190` or `docs/z.md:12`; fenced code blocks and inline
// code spans are blanked before matching, so an example in code never counts.
const pathLine = /[A-Za-z0-9_./-]+\.(?:rs|md|ya?ml|toml|json|tsx?|jsx?|mjs|cjs|sh|go|py|hcl|tpl|txt|lock|css|html?|sql|proto)(?::[0-9]+)+\b/g;

for (const file of markdownFiles(root)) {
  const rel = path.relative(root, file);
  const relPosix = rel.split(path.sep).join('/');
  const src = fs.readFileSync(file, 'utf8');
  const outs = [];
  linksOut.set(relPosix, outs);

  for (const line of unclosedInDepth(src)) failures.push(`${rel}:${line} unclosed ::: in-depth`);

  for (const m of src.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    diagrams++;
    try { await mermaid.parse(m[1]); }
    catch (e) { failures.push(`${rel}:${lineOf(src, m.index)} mermaid: ${String(e.message).split('\n')[0]}`); }
  }

  // Blank out fenced blocks, inline code spans and URLs (keeping line numbers) so an example
  // inside code, or a path that is the tail of a URL, is not read as a `path:line`. A fence is
  // three or more backticks or tildes, closed by at least as many of the same character and
  // nothing after it.
  const blank = (code) => code.replace(/[^\n]/g, ' ');
  function blankCode(text) {
    const lines = text.split('\n');
    let fence = null;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const m = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
      if (fence) {
        lines[i] = blank(line);
        if (m && m[1][0] === fence.ch && m[1].length >= fence.len && m[2].trim() === '') fence = null;
        continue;
      }
      if (m && !(m[1][0] === '`' && m[2].includes('`'))) {
        fence = { ch: m[1][0], len: m[1].length };
        lines[i] = blank(line);
      }
    }
    return lines.join('\n');
  }
  const prose = blankCode(src)
    .replace(/`[^`\n]+`/g, blank)
    .replace(/\bhttps?:\/\/[^\s)>]+/g, blank);
  for (const m of prose.matchAll(/\]\(([^)\s]+)\)/g)) {
    const target = m[1].split('#')[0];
    if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // anchors, http(s):, mailto:
    links++;
    const abs = path.resolve(path.dirname(file), decodeURI(target));
    outs.push(abs);
    if (!fs.existsSync(abs)) {
      failures.push(`${rel}:${lineOf(src, m.index)} broken link: ${m[1]}`);
    }
  }

  // No `path:line` in a page for people (AD-074); the dated records and the still-mixed pages are exempt.
  if (relPosix.startsWith('docs/') && !datedRecords.has(relPosix) && !stillMixed.has(relPosix)) {
    for (const m of prose.matchAll(pathLine)) {
      failures.push(`${rel}:${lineOf(src, m.index)} path:line in a page for people: ${m[0]}`);
    }
  }
}

// A pair shares its relative path and links both ways (AD-074): the agent page of
// `docs/x/y.md` is `.agents/docs/x/y.md`. An agent page with no counterpart is exempt.
for (const [agentRel, outs] of linksOut) {
  if (!agentRel.startsWith('.agents/docs/')) continue;
  const peopleRel = agentRel.slice('.agents/'.length);
  if (!linksOut.has(peopleRel)) continue;
  const peopleAbs = path.resolve(root, peopleRel);
  const agentAbs = path.resolve(root, agentRel);
  if (!linksOut.get(peopleRel).includes(agentAbs)) {
    failures.push(`${peopleRel}:1 pair: no link to its agent page (${agentRel})`);
  }
  if (!outs.includes(peopleAbs)) {
    failures.push(`${agentRel}:1 pair: no link to the page for people (${peopleRel})`);
  }
}

console.log(`${diagrams} diagrams, ${links} relative links checked`);
if (failures.length) {
  console.error(failures.join('\n'));
  console.error(`${failures.length} problem(s)`);
  process.exit(1);
}
console.log('docs OK');
