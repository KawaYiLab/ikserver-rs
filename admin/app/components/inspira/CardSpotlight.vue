<script setup lang="ts">
// 移植自 Inspira UI（card-spotlight）：鼠标在卡片上时，一团柔光跟随指针；移开即消失，没有常驻动效。
// 改动：底色、边框跟随后台主题令牌，光斑默认用强调色的淡色。
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = withDefaults(defineProps<{
  class?: HTMLAttributes['class']
  as?: string
  gradientSize?: number
  gradientColor?: string
}>(), {
  as: 'div',
  gradientSize: 220,
  gradientColor: 'color-mix(in oklch, var(--brand) 18%, transparent)',
})

const x = ref(-9999)
const y = ref(-9999)

function move(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  x.value = event.clientX - rect.left
  y.value = event.clientY - rect.top
}

function leave() {
  x.value = -9999
  y.value = -9999
}

const glow = computed(() =>
  `radial-gradient(${props.gradientSize}px circle at ${x.value}px ${y.value}px, ${props.gradientColor}, transparent 70%)`)
</script>

<template>
  <component
    :is="props.as"
    :class="cn('group/spot relative overflow-hidden rounded-xl border bg-card text-card-foreground', props.class)"
    @mousemove="move"
    @mouseleave="leave"
  >
    <div
      aria-hidden="true"
      class="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover/spot:opacity-100"
      :style="{ background: glow }"
    />
    <div class="relative">
      <slot />
    </div>
  </component>
</template>
