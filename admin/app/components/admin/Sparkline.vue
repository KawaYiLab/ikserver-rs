<script setup lang="ts">
/** 统计卡片里的迷你折线：只表达走势，具体数值在下方主图与数据表里。 */
const props = defineProps<{ values: number[] }>()

const points = computed(() => {
  const n = props.values.length
  if (n < 2) return ''
  const max = Math.max(...props.values, 1)
  return props.values
    .map((v, i) => `${(i / (n - 1)) * 100},${30 - (v / max) * 28 - 1}`)
    .join(' ')
})
</script>

<template>
  <svg viewBox="0 0 100 30" preserveAspectRatio="none" class="h-8 w-full" aria-hidden="true">
    <polyline
      :points="points" fill="none" class="stroke-chart-1" stroke-width="2"
      stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"
    />
  </svg>
</template>
