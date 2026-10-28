<script setup lang="ts">
import type { Component } from 'vue'

/** 统计卡。`value` 是数字时入场滚动一次（Inspira Number Ticker），字符串原样显示；`progress` 给出 0–100 的用量条。 */
defineProps<{
  title: string
  value: string | number
  hint?: string
  icon?: Component
  tone?: 'default' | 'warning' | 'danger'
  progress?: number
}>()
</script>

<template>
  <InspiraCardSpotlight
    class="h-full p-4 shadow-[var(--shadow-card)] transition-colors hover:border-foreground/15"
    :gradient-size="180" gradient-color="color-mix(in oklch, var(--foreground) 5%, transparent)"
  >
    <div class="flex items-center justify-between gap-2 text-xs text-muted-foreground">
      <span>{{ title }}</span>
      <span v-if="icon" class="grid size-6 place-items-center rounded-md border bg-background">
        <component :is="icon" class="size-3.5" />
      </span>
    </div>
    <div
      class="mt-2 text-2xl font-semibold tracking-tight tabular-nums"
      :class="{ 'text-warning': tone === 'warning', 'text-destructive': tone === 'danger' }"
    >
      <InspiraNumberTicker v-if="typeof value === 'number'" :value="value" />
      <template v-else>
        {{ value }}
      </template>
    </div>
    <div v-if="progress !== undefined" class="mt-2 h-1 overflow-hidden rounded-full bg-muted" role="progressbar" :aria-valuenow="progress" aria-valuemin="0" aria-valuemax="100">
      <div class="h-full rounded-full bg-foreground/80 transition-[width] duration-700 ease-out" :style="{ width: `${progress}%` }" />
    </div>
    <p v-if="hint" class="mt-1.5 text-xs text-muted-foreground">
      {{ hint }}
    </p>
  </InspiraCardSpotlight>
</template>
