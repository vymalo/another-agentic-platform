// One page per operation of api/platform-api-v1.openapi.json, found by its operationId, with the permission it needs
// (its `x-permission`; `GET /v1/me` has none beyond a valid token).
import { usePaths } from "vitepress-openapi";
import { assertRouteId } from "../../../.vitepress/openapi/shared";
import { loadSpec } from "../../../.vitepress/openapi/spec";

export default {
  paths() {
    const spec = loadSpec();
    return usePaths({ spec })
      .getPathsByVerbs()
      .map(({ operationId, summary, verb, path }: any) => {
        const method = String(verb).toLowerCase();
        return {
          params: {
            operationId: assertRouteId(operationId, `operationId of ${verb} ${path}`),
            permission: spec.paths?.[path]?.[method]?.["x-permission"] ?? "",
            pageTitle: `${summary ?? `${String(verb).toUpperCase()} ${path}`}`,
          },
        };
      });
  },
};
