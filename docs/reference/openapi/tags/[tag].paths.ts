// One page per tag of api/platform-api-v1.openapi.json (its top-level `tags:`; every operation has exactly one).
import { usePaths } from "vitepress-openapi";
import { assertRouteId } from "../../../.vitepress/openapi/shared";
import { loadSpec } from "../../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec() })
      .getTags()
      .map(({ name }: { name: string }) => ({
        params: { tag: assertRouteId(name, "tag"), pageTitle: name },
      }));
  },
};
