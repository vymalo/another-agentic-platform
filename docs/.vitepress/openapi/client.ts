// Client side of the API pages. Only reference/openapi.md and the tags/ and operations/ routes import this, so the
// contract (a few hundred KB of JSON) is in their chunks and not in the theme that every page loads.
import { useOpenapi } from "vitepress-openapi/client";
import raw from "../../api/platform-api-v1.openapi.json?raw";

import { resolveServers } from "./shared";

const spec = resolveServers(JSON.parse(raw));

// Makes the contract the global spec of the library's components. The theme holds the rest of its configuration.
export function usePlatformApiSpec() {
  useOpenapi({ spec });
}
