---
aside: false
outline: false
title: Platform API tag
---

<script setup>
import { useRoute } from "vitepress";
import { usePlatformApiSpec } from "../../../.vitepress/openapi/client";

usePlatformApiSpec();
const tag = useRoute().data.params.tag;
</script>

<OASpec :tags="[tag]" hide-info hide-servers />
