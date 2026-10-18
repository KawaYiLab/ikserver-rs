<script setup lang="ts">
import { RotateCw, SearchX } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { TableCell, TableEmpty, TableRow } from '@/components/ui/table'

/**
 * 表格的加载 / 空 / 失败状态，放在 `<TableBody>` 末尾。
 * 首次加载显示骨架行；没有数据时按是否有筛选给出不同提示；加载失败可以重试。
 */
withDefaults(defineProps<{
  colspan: number
  loading: boolean
  /** 当前没有可显示的行。 */
  empty: boolean
  failed?: boolean
  /** 有筛选条件生效（空状态时提供「清除筛选」）。 */
  filtered?: boolean
  emptyText?: string
  rows?: number
}>(), { rows: 5, emptyText: '还没有数据' })

defineEmits<{ retry: [], reset: [] }>()
</script>

<template>
  <template v-if="empty">
    <template v-if="loading">
      <TableRow v-for="i in rows" :key="i" class="hover:bg-transparent">
        <TableCell :colspan="colspan">
          <Skeleton class="h-5 w-full" :style="{ maxWidth: `${90 - (i % 3) * 15}%` }" />
        </TableCell>
      </TableRow>
    </template>
    <TableEmpty v-else-if="failed" :colspan="colspan">
      <div class="flex flex-col items-center gap-3 text-muted-foreground">
        <span>加载失败</span>
        <Button size="sm" variant="outline" @click="$emit('retry')">
          <RotateCw class="size-4" />重试
        </Button>
      </div>
    </TableEmpty>
    <TableEmpty v-else :colspan="colspan">
      <div class="flex flex-col items-center gap-3 text-muted-foreground">
        <SearchX v-if="filtered" class="size-6 opacity-60" />
        <span>{{ filtered ? '没有符合条件的结果' : emptyText }}</span>
        <Button v-if="filtered" size="sm" variant="outline" @click="$emit('reset')">清除筛选</Button>
      </div>
    </TableEmpty>
  </template>
</template>
