<script setup lang="ts">
// 移植自 Inspira UI（number-ticker）：进入视口时从 0 滚到目标值，只滚一次；之后值变化时从旧值过渡。
// 改动：去掉固定的黑白文字色，数字格式改用 zh-CN，尊重「减少动态效果」。
import { TransitionPresets, useElementVisibility, usePreferredReducedMotion, useTransition } from '@vueuse/core'
import { cn } from '@/lib/utils'

const props = withDefaults(defineProps<{
  value?: number
  duration?: number
  delay?: number
  decimalPlaces?: number
  class?: string
}>(), {
  value: 0,
  duration: 900,
  delay: 0,
  decimalPlaces: 0,
})

const spanRef = ref<HTMLSpanElement>()
const reduced = usePreferredReducedMotion()
const target = ref(0)
const output = useTransition(target, {
  delay: props.delay,
  duration: computed(() => (reduced.value === 'reduce' ? 0 : props.duration)),
  transition: TransitionPresets.easeOutCubic,
})

const format = computed(() => new Intl.NumberFormat('zh-CN', {
  minimumFractionDigits: props.decimalPlaces,
  maximumFractionDigits: props.decimalPlaces,
}))
const text = computed(() => format.value.format(Number(output.value.toFixed(props.decimalPlaces))))

const visible = useElementVisibility(spanRef)
const seen = ref(false)
const stop = watch(visible, (on) => {
  if (!on || seen.value) return
  seen.value = true
  target.value = props.value
  stop()
}, { immediate: true })
watch(() => props.value, (value) => {
  if (seen.value) target.value = value
})
</script>

<template>
  <span ref="spanRef" :class="cn('inline-block tabular-nums', props.class)">{{ text }}</span>
</template>
