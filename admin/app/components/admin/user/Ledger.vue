<script setup lang="ts">
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { dennyActionLabel, expActionLabel, expBucketLabel, formatNumber, formatTime } from '~/lib/format'

/** 账号详情的丁尼流水 / 经验记录。 */
const props = defineProps<{ userId: string, kind: 'denny' | 'exp' }>()

type Row = Schemas['DennyLogView'] | Schemas['ExpLogView']

const list = useListQuery<Record<string, never>, Schemas['Paged_DennyLogView'] | Schemas['Paged_ExpLogView']>({
  filters: {},
  pageKey: props.kind === 'denny' ? 'dennyPage' : 'expPage',
  fetch: (_, page) => props.kind === 'denny'
    ? unwrap(api.GET('/v1/admin/users/{id}/denny-logs', { params: { path: { id: props.userId }, query: { page, limit: 30 } } }))
    : unwrap(api.GET('/v1/admin/users/{id}/exp-logs', { params: { path: { id: props.userId }, query: { page, limit: 30 } } })),
})
const { result, loading } = list
const items = computed<Row[]>(() => result.value?.items ?? [])

function isDenny(row: Row): row is Schemas['DennyLogView'] {
  return 'balance' in row
}

function signed(amount: number) {
  return amount > 0 ? `+${formatNumber(amount)}` : formatNumber(amount)
}
</script>

<template>
  <div>
    <div class="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>时间</TableHead>
            <TableHead>动作</TableHead>
            <TableHead class="text-right">{{ kind === 'denny' ? '变动' : '经验' }}</TableHead>
            <TableHead class="text-right">{{ kind === 'denny' ? '余额' : '上限桶' }}</TableHead>
            <TableHead class="hidden md:table-cell">{{ kind === 'denny' ? '说明' : '业务日' }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="(row, i) in items" :key="i">
            <TableCell class="whitespace-nowrap">{{ formatTime(row.createdAt) }}</TableCell>
            <template v-if="isDenny(row)">
              <TableCell>{{ dennyActionLabel[row.action] ?? row.action }}</TableCell>
              <TableCell class="text-right tabular-nums" :class="row.amount > 0 ? 'text-success' : 'text-destructive'">{{ signed(row.amount) }}</TableCell>
              <TableCell class="text-right tabular-nums">{{ formatNumber(row.balance) }}</TableCell>
              <TableCell class="hidden md:table-cell max-w-xs truncate text-muted-foreground" :title="row.description">{{ row.description || '—' }}</TableCell>
            </template>
            <template v-else>
              <TableCell>{{ expActionLabel[row.action] ?? row.action }}</TableCell>
              <TableCell class="text-right tabular-nums">
                +{{ formatNumber(row.amount) }}
                <span v-if="row.requested > row.amount" class="text-xs text-muted-foreground">（申请 {{ row.requested }}，触顶）</span>
              </TableCell>
              <TableCell class="text-right">{{ expBucketLabel[row.bucket] ?? row.bucket }}</TableCell>
              <TableCell class="hidden md:table-cell text-muted-foreground">{{ row.bizDay }}</TableCell>
            </template>
          </TableRow>
          <AdminListState
            :colspan="5" :loading="loading" :failed="list.failed.value"
            :empty="!items.length" :empty-text="kind === 'denny' ? '还没有丁尼流水' : '还没有经验记录'"
            @retry="list.refresh"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" @change="list.setPage" />
  </div>
</template>
