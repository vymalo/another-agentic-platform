---
aside: false
outline: false
title: Platform API reference
---

<script setup>
import { usePlatformApiSpec } from "../.vitepress/openapi/client";
import PermissionsTable from "../.vitepress/theme/PermissionsTable.vue";

usePlatformApiSpec();
</script>

# Platform API reference

The whole contract, [`platform-api-v1.openapi.json`](../api/platform-api-v1.openapi.json) (OpenAPI 3.1), rendered from the file at build time. One page per tag and one per operation are linked from the sidebar, and each operation's page names the permission it needs.

## Permissions

Every operation and the one permission it needs (`x-permission` in the contract). Every route needs a valid token first; `GET /v1/me` needs no permission beyond it.

<PermissionsTable />

## The contract

<OASpec />
