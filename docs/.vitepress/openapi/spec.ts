// The Platform API's contract, read from the one file that is the contract (api/platform-api-v1.openapi.json).
// Node side only (the sidebar and the dynamic routes); the pages read the same file in openapi/client.ts.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { resolveServers } from "./shared";

const file = fileURLToPath(new URL("../../api/platform-api-v1.openapi.json", import.meta.url));

export function loadSpec(): any {
  return resolveServers(JSON.parse(readFileSync(file, "utf8")));
}
