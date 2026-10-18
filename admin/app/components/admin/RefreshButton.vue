<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import { Button } from '@/components/ui/button'

defineProps<{ loading?: boolean }>()
defineEmits<{ click: [] }>()
</script>

<template>
  <Button variant="outline" size="sm" :disabled="loading" @click="$emit('click')">
    <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />刷新
  </Button>
</template>
