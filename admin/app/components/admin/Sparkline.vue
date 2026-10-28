<script setup lang="ts">
/** 统计卡片里的迷你折线：只表达走势，具体数值在下方主图与数据表里。 */
const props = defineProps<{ values: number[] }>()

const id = useId()
const coords = computed(() => {
  const n = props.values.length
  if (n < 2) return []
  const max = Math.max(...props.values, 1)
  return props.values.map((v, i) => [(i / (n - 1)) * 100, 30 - (v / max) * 26 - 2] as const)
})
const line = computed(() => coords.value.map(([x, y]) => `${x},${y}`).join(' '))
const area = computed(() => (coords.value.length ? `M0,30 L${line.value.replaceAll(' ', ' L')} L100,30 Z` : ''))
</script>

<template>
  <svg viewBox="0 0 100 30" preserveAspectRatio="none" class="h-8 w-full overflow-visible" aria-hidden="true">
    <defs>
      <linearGradient :id="`spark-${id}`" x1="0" x2="0" y1="0" y2="1">
        <stop offset="0%" stop-color="currentColor" stop-opacity="0.16" />
        <stop offset="100%" stop-color="currentColor" stop-opacity="0" />
      </linearGradient>
    </defs>
    <path :d="area" :fill="`url(#spark-${id})`" />
    <polyline
      :points="line" fill="none" stroke="currentColor" stroke-width="1.5"
      stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"
    />
  </svg>
</template>
