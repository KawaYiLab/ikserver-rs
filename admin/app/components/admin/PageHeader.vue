<script setup lang="ts">
import { navItemFor } from '~/lib/nav'

defineProps<{ title: string, description?: string }>()

// 标题旁的图标与侧边栏一致。
const route = useRoute()
const icon = computed(() => navItemFor(route.path)?.icon)
</script>

<template>
  <div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
    <div class="flex items-start gap-3">
      <span v-if="icon" class="mt-0.5 hidden size-10 shrink-0 place-items-center rounded-lg border bg-card shadow-xs sm:grid">
        <component :is="icon" class="size-5" />
      </span>
      <div>
        <h1 class="text-2xl font-semibold tracking-tight">
          {{ title }}
        </h1>
        <p v-if="description" class="mt-1 text-sm text-muted-foreground">
          {{ description }}
        </p>
      </div>
    </div>
    <div class="flex flex-wrap items-center gap-2">
      <slot />
    </div>
  </div>
</template>
