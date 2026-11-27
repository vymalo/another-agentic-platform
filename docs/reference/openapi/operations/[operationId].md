---
aside: false
outline: false
title: Platform API operation
---

<script setup>
import { useRoute } from "vitepress";
import { usePlatformApiSpec } from "../../../.vitepress/openapi/client";

usePlatformApiSpec();
const { operationId, permission } = useRoute().data.params;
</script>

<p v-if="permission">Needs <code v-text="permission"></code>.</p>

<OAOperation :operationId="operationId" />
