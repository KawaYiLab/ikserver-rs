<script setup lang="ts">
import logo from '~/assets/brand/logo.png'
import { cn } from '@/lib/utils'

// 绳网 logo（与主站同一张白色图标），放在深色圆角底上，浅色 / 深色主题都清晰。
defineProps<{ class?: string }>()
</script>

<template>
  <span :class="cn('grid size-8 shrink-0 place-items-center rounded-lg bg-zinc-950 shadow-[inset_0_0_0_1px_oklch(1_0_0/10%)]', $props.class)">
    <img :src="logo" alt="" class="size-[78%] select-none" draggable="false">
  </span>
</template>
