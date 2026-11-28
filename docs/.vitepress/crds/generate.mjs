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
// For a cell that holds code: its spaces are part of the value. Only a line break goes, because it would end the row.
const escapeCell = (text) => String(text ?? "").replace(/\|/g, "\\|").replace(/[\r\n]+/g, " ");
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
    if (rule.message) parts.push(`rule: ${String(rule.message).replace(/\s+/g, " ").trim()}`);
  }
  return parts.join("; ");
}

function row(path, schema, required) {
  const description = String(schema.description ?? "").replace(/\s+/g, " ").trim();
  const shown = description.length > DESCRIPTION_MAX ? `${description.slice(0, DESCRIPTION_MAX - 1)}…` : description;
  const cells = [
    code(path),
    typeOf(schema),
    required ? "yes" : "no",
    schema.default === undefined ? "—" : code(JSON.stringify(schema.default)),
    boundsOf(schema) || "—",
    shown,
  ];
  // Each cell is built raw and escaped here, once: a pipe from the schema never ends a cell.
  return `| ${cells.map(escapeCell).join(" | ")} |`;
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
  if (value && typeof value === "object" && value.properties) {
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

export function page(kind, crd) {
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

/** A made-up CRD whose `spec` is `spec`, so a case can ask for a shape no checked-in CRD has. */
function probe(spec) {
  const schema = { description: "probe", required: ["spec"], properties: { spec } };
  return {
    spec: {
      group: "agents.vymalo.com",
      names: { kind: "Probe", plural: "probes" },
      scope: "Namespaced",
      versions: [{ name: "v1alpha1", schema: { openAPIV3Schema: schema } }],
    },
  };
}

/** The rows of the `spec` table of a probe's page, without the header and its rule. */
function specTable(spec) {
  const section = page("Probe", probe(spec)).split("## `spec`\n\n")[1] ?? "";
  return section.split("\n\n")[0].split("\n").slice(2);
}

/** A row as a Markdown table reads it: only a pipe with no backslash before it ends a cell. */
const splitRow = (row) => row.split(/(?<!\\)\|/).slice(1, -1).map((text) => text.trim());

/** One row, six cells, and in cell `index` the source `a|b` escaped exactly once. */
function pipeCase(name, field, index) {
  const rows = specTable({ type: "object", properties: { field } });
  const cells = splitRow(rows[0] ?? "");
  const text = cells[index] ?? "";
  const ok = rows.length === 1 && cells.length === 6 && text.includes("a\\|b") && !text.includes("\\\\|");
  return { name, ok, detail: `${cells.length} cells in ${rows[0]}` };
}

/** One row of six cells, and the source's two spaces still two in cell `index`. */
function spacesCase(name, field, index, source) {
  const rows = specTable({ type: "object", properties: { field } });
  const cells = splitRow(rows[0] ?? "");
  const ok = rows.length === 1 && cells.length === 6 && (cells[index] ?? "").includes(source);
  return { name, ok, detail: rows.join(" / ") };
}

/** What the generator does with shapes the checked-in CRDs do not have, as `{ name, ok, detail }`. */
export function unitCases() {
  const [pattern, choice, fallback, description, rule] = [
    pipeCase("a pattern with a pipe stays in its cell", { type: "string", pattern: "^(a|b)$" }, 4),
    pipeCase("an enum value with a pipe stays in its cell", { type: "string", enum: ["a|b"] }, 4),
    pipeCase("a string default with a pipe stays in its cell", { type: "string", default: "a|b" }, 3),
    pipeCase("a description with a pipe in a code span stays in its cell", { type: "string", description: "the `a|b` value" }, 5),
    pipeCase(
      "a rule message with a pipe is escaped once",
      { type: "string", "x-kubernetes-validations": [{ rule: "true", message: "a|b" }] },
      4,
    ),
  ];

  const mapRows = specTable({
    type: "object",
    properties: { known: { type: "string" } },
    additionalProperties: { type: "object", properties: { free: { type: "string" } } },
  });
  const paths = mapRows.map((row) => splitRow(row)[0]);
  const map = {
    name: "a map beside fixed fields keeps the rows of its value",
    ok: ["spec.known", "spec{}", "spec{}.free"].every((path) => paths.includes(`<code>${path}</code>`)),
    detail: `paths: ${paths.join(", ")}`,
  };

  const plainRows = specTable({ type: "object", properties: { field: { type: "string" } } });
  const plain = {
    name: "control: a plain string is one row of six cells",
    ok: plainRows.length === 1 && splitRow(plainRows[0]).length === 6,
    detail: plainRows.join(" / "),
  };

  const spaced = [
    spacesCase("a pattern keeps its two spaces", { type: "string", pattern: "^a  b$" }, 4, "^a  b$"),
    spacesCase("a string default keeps its two spaces", { type: "string", default: "a  b" }, 3, '"a  b"'),
  ];

  return [pattern, choice, fallback, description, rule, map, plain, ...spaced];
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
