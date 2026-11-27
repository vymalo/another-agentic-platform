// Shared by the node loader (openapi/spec.ts), the config and the client (openapi/client.ts): no node APIs here.

// The site's base, written once: config.ts reads DOCS_BASE with this default.
export const DEFAULT_BASE = "/another-agentic-platform/";

// A route id or tag becomes a path segment of a built page: nothing else may.
export function assertRouteId(id: unknown, what: string): string {
  if (typeof id !== "string" || !/^[A-Za-z0-9_-]+$/.test(id)) {
    throw new Error(`${what} ${JSON.stringify(id)} is not a safe route segment ([A-Za-z0-9_-]+)`);
  }
  return id;
}

// The contract names its server as `{base}`, with a variable the deployment sets (the chart's Ingress host). The
// renderer does not substitute server variables, so resolve each one from its default before it renders: the site
// shows the example host and says it is configuration. A page of the site only; the file is untouched.
export function resolveServers<T>(spec: T): T {
  const s = spec as { servers?: unknown } | null;
  if (!Array.isArray(s?.servers)) return spec;
  return {
    ...(s as object),
    servers: (s.servers as { url?: unknown; variables?: Record<string, { default?: unknown }> }[]).map((server) => {
      let url = String(server.url ?? "");
      for (const [name, variable] of Object.entries(server.variables ?? {})) {
        url = url.replaceAll(`{${name}}`, String(variable?.default ?? `{${name}}`));
      }
      const { variables, ...rest } = server;
      void variables;
      return { ...rest, url };
    }),
  } as T;
}
