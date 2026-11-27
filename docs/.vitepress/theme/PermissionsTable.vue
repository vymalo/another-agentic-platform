<script setup lang="ts">
// The permission of every operation, generated from the contract itself (its `x-permission`), so the reference page
// cannot disagree with the document. Read here rather than from `openapi/client.ts`: the table is on one page, and the
// page already loads the spec for the renderer.
import raw from "../../api/platform-api-v1.openapi.json?raw";

const spec = JSON.parse(raw);
const verbs = ["get", "put", "post", "delete", "patch", "head", "options", "trace"];
const rows: { method: string; path: string; permission: string }[] = [];
for (const [path, operations] of Object.entries(spec.paths ?? {})) {
  for (const [method, operation] of Object.entries(operations as Record<string, unknown>)) {
    if (!verbs.includes(method)) continue;
    const permission = (operation as Record<string, unknown>)["x-permission"];
    rows.push({
      method: method.toUpperCase(),
      path,
      permission: typeof permission === "string" && permission !== "" ? permission : "a valid token",
    });
  }
}
rows.sort((a, b) => a.path.localeCompare(b.path) || a.method.localeCompare(b.method));
</script>

<template>
  <table>
    <thead>
      <tr>
        <th>Method</th>
        <th>Path</th>
        <th>Permission</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="row in rows" :key="`${row.method} ${row.path}`">
        <td><code>{{ row.method }}</code></td>
        <td><code>{{ row.path }}</code></td>
        <td><code>{{ row.permission }}</code></td>
      </tr>
    </tbody>
  </table>
</template>
