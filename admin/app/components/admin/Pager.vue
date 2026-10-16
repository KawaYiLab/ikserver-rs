<script setup lang="ts">
import { ChevronLeft, ChevronRight } from '@lucide/vue'
import { Button } from '@/components/ui/button'

const props = defineProps<{ page: number, limit: number, total: number, unit?: string }>()
const emit = defineEmits<{ change: [page: number] }>()
const pages = computed(() => Math.max(1, Math.ceil(props.total / props.limit)))
</script>

<template>
  <div class="flex items-center justify-between gap-2 py-3 text-sm text-muted-foreground">
    <span>共 {{ total }} {{ unit ?? '条' }} · 第 {{ page }} / {{ pages }} 页</span>
    <div class="flex gap-2">
      <Button variant="outline" size="sm" :disabled="page <= 1" @click="emit('change', page - 1)">
        <ChevronLeft class="size-4" />上一页
      </Button>
      <Button variant="outline" size="sm" :disabled="page >= pages" @click="emit('change', page + 1)">
        下一页<ChevronRight class="size-4" />
      </Button>
    </div>
  </div>
</template>
