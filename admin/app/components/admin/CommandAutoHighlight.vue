<script setup lang="ts">
import { injectListboxRootContext } from 'reka-ui'

/**
 * 放在 `<Command>` 里：`watch` 的内容变了（异步搜索结果到达）就高亮第一项，
 * 这样输入后直接回车即可打开。reka-ui 只在输入那一刻高亮，结果晚到时会落空。
 */
const props = defineProps<{ watch: unknown }>()
const root = injectListboxRootContext()

watch(() => props.watch, () => {
  nextTick(() => root.highlightFirstItem())
}, { flush: 'post' })
</script>

<template>
  <slot />
</template>
