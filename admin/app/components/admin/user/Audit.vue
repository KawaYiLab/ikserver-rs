<script setup lang="ts">
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import { formatTime } from '~/lib/format'
import { auditActionLabel } from '~/lib/audit'

/** 账号详情的「操作记录」：后台对这个账号做过的修改（只有管理员）。 */
const props = defineProps<{ userId: string }>()

const list = useListQuery({
  filters: {},
  pageKey: 'auditPage',
  fetch: (_, page) => unwrap(api.GET('/v1/admin/audit', {
    params: { query: { targetType: 'user', targetId: props.userId, page, limit: 30 } },
  })),
})
const { result, loading } = list
</script>

<template>
  <div>
    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>时间</TableHead>
            <TableHead>操作人</TableHead>
            <TableHead>动作</TableHead>
            <TableHead class="hidden md:table-cell">详情</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="entry in result?.items ?? []" :key="entry.id">
            <TableCell class="whitespace-nowrap">{{ formatTime(entry.createdAt) }}</TableCell>
            <TableCell>{{ entry.actorName ? `@${entry.actorName}` : entry.actorId ?? '(已删除)' }}</TableCell>
            <TableCell>{{ auditActionLabel[entry.action] ?? entry.action }}</TableCell>
            <TableCell class="hidden md:table-cell max-w-md truncate font-mono text-xs text-muted-foreground" :title="JSON.stringify(entry.details)">
              {{ JSON.stringify(entry.details) }}
            </TableCell>
          </TableRow>
          <AdminListState
            :colspan="4" :loading="loading" :failed="list.failed.value" :empty="!result?.items.length"
            empty-text="后台还没有对这个账号做过修改" @retry="list.refresh"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" @change="list.setPage" />
  </div>
</template>
