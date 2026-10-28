<script setup lang="ts">
/** 首次出现时上浮淡入一次（按 `index` 依次错开）。系统开启「减少动态效果」时直接显示。 */
withDefaults(defineProps<{ index?: number, as?: 'div' | 'section' }>(), { index: 0, as: 'div' })
</script>

<template>
  <Motion
    :as="as"
    :initial="{ opacity: 0, y: 8, filter: 'blur(2px)' }"
    :animate="{ opacity: 1, y: 0, filter: 'blur(0px)' }"
    :transition="{ duration: 0.36, delay: index * 0.05, ease: [0.22, 1, 0.36, 1] }"
  >
    <slot />
  </Motion>
</template>
