<script setup lang="ts">
import type { HTMLAttributes } from "vue"
import { cn } from "@/lib/utils"

const props = defineProps<{
  class?: HTMLAttributes["class"]
}>()
</script>

<template>
  <tr
    data-slot="table-row"
    :class="cn('hover:bg-muted/40 data-[state=selected]:bg-muted/70 border-b last:border-b-0 transition-colors', props.class)"
  >
    <slot />
  </tr>
</template>
