// Generates one Reference page per CRD kind from the checked-in CRDs, and the drift check that holds them to it.
//
// `deploy/crds/agents.vymalo.com.yaml` is the source of truth (`cargo run -p aap-operator -- crdgen`, and a CI test
// fails when the file drifts from the command). The pages are committed so the site's build reads only the site;
// `npm run check-crds` fails when the CRDs change and the pages do not, and `npm run crds` rewrites them.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseAllDocuments } from "yaml";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "../../..");

/** The checked-in CRDs, the one file that is the schema. */
export const CRDS = join(root, "deploy/crds/agents.vymalo.com.yaml");
/** Where the generated pages live. */
export const OUT = join(root, "docs/reference/crds");

const BOUNDS = [
  "format",
  "minLength",
  "maxLength",
  "minItems",
  "maxItems",
  "minProperties",
  "maxProperties",
  "pattern",
  "enum",
  "minimum",
  "maximum",
];
// A table cell is read, not stored: the whole description is in the CRD the page links.
const DESCRIPTION_MAX = 260;
const MAX_DEPTH = 12;

const cell = (text) => String(text ?? "").replace(/\|/g, "\\|").replace(/\s+/g, " ").trim();
const code = (text) =>
  `<code>${String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")}</code>`;

function typeOf(schema) {
  if (!schema || typeof schema !== "object") return "—";
  if (Array.isArray(schema.oneOf) || Array.isArray(schema.anyOf)) {
    const alternatives = (schema.oneOf ?? schema.anyOf).map(typeOf).filter((t) => t !== "—");
    return [...new Set(alternatives)].join(" or ") || "—";
  }
  const type = schema.type ?? (schema.properties ? "object" : schema.items ? "array" : undefined);
  if (type === "array") return `array of ${typeOf(schema.items)}`;
  if (type === "object" && schema.additionalProperties && typeof schema.additionalProperties === "object") {
    return `map of ${typeOf(schema.additionalProperties)}`;
  }
  return type ?? "—";
}

function boundsOf(schema) {
  const parts = [];
  for (const keyword of BOUNDS) {
    if (schema[keyword] === undefined) continue;
    if (keyword === "enum") {
      parts.push(`one of ${schema[keyword].map((v) => code(JSON.stringify(v))).join(", ")}`);
    } else if (keyword === "pattern") {
      parts.push(`matches ${code(schema[keyword])}`);
    } else {
      parts.push(`${keyword} ${code(schema[keyword])}`);
    }
  }
  for (const rule of schema["x-kubernetes-validations"] ?? []) {
    if (rule.message) parts.push(`rule: ${cell(rule.message)}`);
  }
  return parts.join("; ");
}

function row(path, schema, required) {
  const description = cell(schema.description);
  const shown = description.length > DESCRIPTION_MAX ? `${description.slice(0, DESCRIPTION_MAX - 1)}…` : description;
  return `| ${code(path)} | ${typeOf(schema)} | ${required ? "yes" : "no"} | ${
    schema.default === undefined ? "—" : code(JSON.stringify(schema.default))
  } | ${boundsOf(schema) || "—"} | ${shown} |`;
}

/** Every field under `schema`, as one flat table: the path, its type, whether its parent requires it, its default, its
 * bounds and its description. `[]` is an array's element, `{}` a map's value. */
function walk(schema, path, rows, depth) {
  if (!schema || typeof schema !== "object" || depth > MAX_DEPTH) return;
  const required = Array.isArray(schema.required) ? schema.required : [];
  for (const [name, child] of Object.entries(schema.properties ?? {})) {
    const childPath = `${path}.${name}`;
    rows.push(row(childPath, child, required.includes(name)));
    walk(child, childPath, rows, depth + 1);
  }
  if (schema.items && typeof schema.items === "object") {
    walk(schema.items, `${path}[]`, rows, depth + 1);
  }
  const value = schema.additionalProperties;
  if (value && typeof value === "object" && !schema.properties && value.properties) {
    const valuePath = `${path}{}`;
    rows.push(row(valuePath, value, false));
    walk(value, valuePath, rows, depth + 1);
  }
}

function table(rows) {
  return ["| Field | Type | Required | Default | Bounds | Description |", "|---|---|---|---|---|---|", ...rows].join(
    "\n",
  );
}

function page(kind, crd) {
  const version = crd.spec.versions[0];
  const schema = version.schema.openAPIV3Schema;
  const specRows = [];
  walk(schema.properties?.spec ?? {}, "spec", specRows, 0);
  const statusRows = [];
  if (schema.properties?.status) walk(schema.properties.status, "status", statusRows, 0);
  const specRequired = Array.isArray(schema.required) && schema.required.includes("spec");
  const scope = crd.spec.scope === "Namespaced" ? "namespaced" : "cluster-scoped";
  return [
    "---",
    `sidebarTitle: "${kind}"`,
    "---",
    "",
    `# ${kind}`,
    "",
    cell(schema.description),
    "",
    `The object is \`${crd.spec.group}/${version.name}\`, ${scope}, kind \`${crd.spec.names.kind}\`, plural \`${crd.spec.names.plural}\`. ${
      specRequired ? "`spec` is required." : "`spec` is optional."
    } The table is the whole schema, one row per field path: \`[]\` is an array's element, \`{}\` a map's value. A Kubernetes type's own fields are included, because the CRD admits them.`,
    "",
    "## `spec`",
    "",
    table(specRows),
    "",
    ...(statusRows.length
      ? [
          "::: in-depth The status the operator writes",
          "",
          "Only the operator writes `status`; an object a person applies carries none. It is here because `kubectl get -o yaml` shows it.",
          "",
          table(statusRows),
          "",
          ":::",
          "",
        ]
      : []),
    "The CRD itself, with every description whole, is [`deploy/crds/agents.vymalo.com.yaml`](https://github.com/vymalo/another-agentic-platform/blob/main/deploy/crds/agents.vymalo.com.yaml).",
    "",
  ].join("\n");
}

/** The pages, in the CRD file's order: `{ file, content }` per kind. */
export function renderAll() {
  const pages = [];
  for (const document of parseAllDocuments(readFileSync(CRDS, "utf8"))) {
    const crd = document.toJS();
    if (!crd || crd.kind !== "CustomResourceDefinition") continue;
    const kind = crd.spec.names.kind;
    pages.push({ file: join(OUT, `${kind}.md`), content: page(kind, crd) });
  }
  return pages;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  mkdirSync(OUT, { recursive: true });
  for (const generated of renderAll()) {
    writeFileSync(generated.file, generated.content);
    console.log(`wrote ${generated.file}`);
  }
}
