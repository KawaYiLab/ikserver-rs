<script setup lang="ts">
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatTime } from '~/lib/format'
import { auditActionLabel as auditActions, auditTargetLabel as auditTargets } from '~/lib/audit'

definePageMeta({ admin: true })

const list = useListQuery({
  filters: { action: 'all', targetType: 'all', targetId: '', actorId: '' },
  text: ['targetId', 'actorId'],
  fetch: (f, page) => unwrap(api.GET('/v1/admin/audit', {
    params: {
      query: {
        actorId: optional(f.actorId),
        action: optional(f.action),
        targetType: optional(f.targetType),
        targetId: optional(f.targetId),
        page,
        limit: 50,
      },
    },
  })),
})
const { filters, result, loading } = list
const expanded = ref<string | null>(null)

function details(entry: Schemas['AuditEntry']) {
  return JSON.stringify(entry.details, null, 2)
}

function hasDetails(entry: Schemas['AuditEntry']) {
  const value = entry.details as unknown
  return !!value && typeof value === 'object' && Object.keys(value).length > 0
}
</script>

<template>
  <div>
    <AdminPageHeader title="审计日志" description="后台的每一次修改都会记录操作人、时间、来源与变更内容。">
      <AdminRefreshButton :loading="loading" @click="list.refresh" />
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
      <Select v-model="filters.action">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部动作</SelectItem>
          <SelectItem v-for="(label, key) in auditActions" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.targetType">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部对象</SelectItem>
          <SelectItem v-for="(label, key) in auditTargets" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Input v-model="filters.targetId" placeholder="对象 ID" />
      <Input v-model="filters.actorId" placeholder="操作人 ID" />
    </div>

    <div class="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>时间</TableHead>
            <TableHead>操作人</TableHead>
            <TableHead>动作</TableHead>
            <TableHead>对象</TableHead>
            <TableHead class="hidden lg:table-cell">来源 IP</TableHead>
            <TableHead class="text-right">详情</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <template v-for="entry in result?.items ?? []" :key="entry.id">
            <TableRow>
              <TableCell class="whitespace-nowrap">{{ formatTime(entry.createdAt) }}</TableCell>
              <TableCell>
                <template v-if="entry.actorName">@{{ entry.actorName }}</template>
                <span v-else class="text-muted-foreground">{{ entry.actorId ?? '(已删除)' }}</span>
              </TableCell>
              <TableCell>
                {{ auditActions[entry.action] ?? entry.action }}
                <div class="font-mono text-xs text-muted-foreground">{{ entry.action }}</div>
              </TableCell>
              <TableCell>
                {{ auditTargets[entry.targetType] ?? entry.targetType }}
                <span v-if="entry.targetId" class="font-mono text-xs text-muted-foreground"> {{ entry.targetId }}</span>
              </TableCell>
              <TableCell class="hidden lg:table-cell font-mono text-xs" :title="entry.userAgent ?? ''">{{ entry.ip ?? '—' }}</TableCell>
              <TableCell class="text-right">
                <Button v-if="hasDetails(entry)" size="sm" variant="ghost" @click="expanded = expanded === entry.id ? null : entry.id">
                  {{ expanded === entry.id ? '收起' : '展开' }}
                </Button>
              </TableCell>
            </TableRow>
            <TableRow v-if="expanded === entry.id">
              <TableCell colspan="6" class="bg-muted/50">
                <pre class="max-h-80 overflow-auto text-xs whitespace-pre-wrap">{{ details(entry) }}</pre>
                <div v-if="entry.userAgent" class="mt-2 text-xs text-muted-foreground">UA：{{ entry.userAgent }}</div>
              </TableCell>
            </TableRow>
          </template>
          <AdminListState
            :colspan="6" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" empty-text="还没有审计记录"
            @retry="list.refresh" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" @change="list.setPage" />
  </div>
</template>
