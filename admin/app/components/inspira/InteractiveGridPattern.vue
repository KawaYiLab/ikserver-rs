<script setup lang="ts">
// 移植自 Inspira UI（interactive-grid-pattern）：静态网格，指针经过的格子短暂高亮后慢慢褪去。
// 改动：颜色走主题令牌，高亮用强调色。
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = withDefaults(defineProps<{
  class?: HTMLAttributes['class']
  size?: number
  squares?: [number, number]
}>(), {
  size: 40,
  squares: () => [48, 30],
})

const columns = computed(() => props.squares[0])
const total = computed(() => props.squares[0] * props.squares[1])
const hovered = ref<number | null>(null)
</script>

<template>
  <svg
    aria-hidden="true"
    :width="props.size * props.squares[0]"
    :height="props.size * props.squares[1]"
    :class="cn('absolute inset-0', props.class)"
  >
    <rect
      v-for="(_, index) in total"
      :key="index"
      :x="(index % columns) * props.size"
      :y="Math.floor(index / columns) * props.size"
      :width="props.size"
      :height="props.size"
      :class="cn(
        'stroke-foreground/[0.07] transition-[fill] duration-100 not-[&:hover]:duration-1000',
        hovered === index ? 'fill-brand/25' : 'fill-transparent',
      )"
      @mouseenter="hovered = index"
      @mouseleave="hovered = null"
    />
  </svg>
</template>
