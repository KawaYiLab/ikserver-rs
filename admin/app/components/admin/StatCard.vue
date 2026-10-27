<script setup lang="ts">
import type { Component } from 'vue'

/** 统计卡。`value` 是数字时入场滚动一次（Inspira Number Ticker），字符串原样显示。 */
defineProps<{
  title: string
  value: string | number
  hint?: string
  icon?: Component
  tone?: 'default' | 'warning' | 'danger'
}>()
</script>

<template>
  <InspiraCardSpotlight class="h-full p-5 shadow-sm">
    <div class="flex items-center justify-between gap-2 text-sm text-muted-foreground">
      <span>{{ title }}</span>
      <component :is="icon" v-if="icon" class="size-4" />
    </div>
    <div
      class="mt-3 text-3xl font-semibold tracking-tight tabular-nums"
      :class="{ 'text-warning': tone === 'warning', 'text-destructive': tone === 'danger' }"
    >
      <InspiraNumberTicker v-if="typeof value === 'number'" :value="value" />
      <template v-else>
        {{ value }}
      </template>
    </div>
    <p v-if="hint" class="mt-1 text-xs text-muted-foreground">
      {{ hint }}
    </p>
  </InspiraCardSpotlight>
</template>
