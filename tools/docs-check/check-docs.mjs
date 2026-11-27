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

for (const file of markdownFiles(root)) {
  const rel = path.relative(root, file);
  const src = fs.readFileSync(file, 'utf8');

  for (const line of unclosedInDepth(src)) failures.push(`${rel}:${line} unclosed ::: in-depth`);

  for (const m of src.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    diagrams++;
    try { await mermaid.parse(m[1]); }
    catch (e) { failures.push(`${rel}:${lineOf(src, m.index)} mermaid: ${String(e.message).split('\n')[0]}`); }
  }

  // Blank out fenced blocks and inline code spans (keeping line numbers) so
  // example links inside code are not checked.
  const blank = (code) => code.replace(/[^\n]/g, ' ');
  const prose = src
    .replace(/```[\s\S]*?```/g, blank)
    .replace(/`[^`\n]+`/g, blank);
  for (const m of prose.matchAll(/\]\(([^)\s]+)\)/g)) {
    const target = m[1].split('#')[0];
    if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // anchors, http(s):, mailto:
    links++;
    if (!fs.existsSync(path.resolve(path.dirname(file), decodeURI(target)))) {
      failures.push(`${rel}:${lineOf(src, m.index)} broken link: ${m[1]}`);
    }
  }
}

console.log(`${diagrams} diagrams, ${links} relative links checked`);
if (failures.length) {
  console.error(failures.join('\n'));
  console.error(`${failures.length} problem(s)`);
  process.exit(1);
}
console.log('docs OK');
