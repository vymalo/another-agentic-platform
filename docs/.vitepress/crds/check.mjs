// The drift check of the generated CRD pages: `npm run check-crds`. It fails when a CRD changed and its page did not
// (`npm run crds` rewrites them), and when a page exists that no CRD makes. It then runs the generator's unit cases, the
// shapes no checked-in CRD has.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { renderAll, unitCases } from "./generate.mjs";

const pages = renderAll();
const expected = new Set(pages.map((page) => page.file));
let drift = 0;

for (const page of pages) {
  const actual = existsSync(page.file) ? readFileSync(page.file, "utf8") : "";
  if (actual !== page.content) {
    console.error(`drift: ${page.file}`);
    drift = 1;
  }
}

const directory = pages[0] ? pages[0].file.replace(/\/[^/]+$/, "") : null;
if (directory && existsSync(directory)) {
  for (const name of readdirSync(directory)) {
    const file = `${directory}/${name}`;
    if (name.endsWith(".md") && !expected.has(file)) {
      console.error(`drift: ${file} has no CRD`);
      drift = 1;
    }
  }
}

if (drift) {
  console.error("run `npm --prefix docs run crds` and commit the pages");
  process.exit(1);
}
console.log(`the ${pages.length} CRD pages are what the CRDs say`);

let failed = 0;
for (const unit of unitCases()) {
  console.log(`${unit.ok ? "ok  " : "FAIL"} ${unit.name}`);
  if (!unit.ok) {
    console.error(`     ${unit.detail}`);
    failed += 1;
  }
}
if (failed) {
  console.error(`${failed} unit case(s) of the CRD page generator failed (docs/.vitepress/crds/generate.mjs)`);
  process.exit(1);
}
