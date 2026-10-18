<script setup lang="ts">
import { ExternalLink, MonitorSmartphone, ShieldAlert, UserRound } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatTime, roleLabel, roleVariant, statusLabel, userStatusVariant } from '~/lib/format'
import { userPath } from '~/lib/links'

type User = Schemas['AdminUser']

const { isAdmin } = useSession()
const siteLink = useSiteLink()

const list = useListQuery({
  filters: { q: '', role: 'all', status: 'all', ai: 'all' },
  text: ['q'],
  fetch: (f, page) => unwrap(api.GET('/v1/admin/users', {
    params: {
      query: {
        q: optional(f.q),
        role: optional(f.role),
        status: optional(f.status),
        ai: optionalBool(f.ai),
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading } = list
const load = list.refresh

function rowActions(user: User): RowAction[] {
  return [
    { label: '查看详情', icon: UserRound, onSelect: () => navigateTo(userPath(user.id)) },
    { label: '登录设备', icon: MonitorSmartphone, hidden: !isAdmin.value, onSelect: () => navigateTo(userPath(user.id, 'sessions')) },
    { label: '举报与处罚', icon: ShieldAlert, onSelect: () => navigateTo(userPath(user.id, 'moderation')) },
    { label: '在主站打开主页', icon: ExternalLink, href: siteLink(`/profile/${user.id}`) },
  ]
}

</script>

<template>
  <div>
    <AdminPageHeader title="账号" :description="isAdmin ? '调整角色、丁尼和经验都会记入审计日志。' : '版主可以查看账号并封禁 / 解封；其他调整需要管理员。'">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-5">
      <Input v-model="filters.q" placeholder="用户名 / 邮箱 / ID" class="lg:col-span-2" />
      <Select v-model="filters.role">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部角色</SelectItem>
          <SelectItem v-for="(label, key) in roleLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.status">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem v-for="(label, key) in statusLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.ai">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部账号</SelectItem>
          <SelectItem value="yes">只看 AI 账号</SelectItem>
          <SelectItem value="no">排除 AI 账号</SelectItem>
        </SelectContent>
      </Select>
    </div>

    <div class="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>用户</TableHead>
            <TableHead class="hidden lg:table-cell">邮箱</TableHead>
            <TableHead>角色</TableHead>
            <TableHead class="hidden md:table-cell text-right">等级</TableHead>
            <TableHead class="hidden md:table-cell">考试</TableHead>
            <TableHead>状态</TableHead>
            <TableHead class="hidden lg:table-cell">注册时间</TableHead>
            <TableHead class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="user in result?.items ?? []" :key="user.id">
            <TableCell>
              <NuxtLink :to="userPath(user.id)" class="font-medium hover:underline">@{{ user.username }}</NuxtLink>
              <Badge v-if="user.isAiAgent" variant="secondary" class="ml-2">AI</Badge>
              <div class="text-xs text-muted-foreground">ID {{ user.id }}</div>
            </TableCell>
            <TableCell class="hidden lg:table-cell">{{ user.email ?? '—' }}</TableCell>
            <TableCell>
              <Badge :variant="roleVariant(user.role)">{{ roleLabel[user.role] }}</Badge>
            </TableCell>
            <TableCell class="hidden md:table-cell text-right tabular-nums">Lv.{{ user.level }}</TableCell>
            <TableCell class="hidden md:table-cell">
              <span v-if="user.examPassedAt">已通过</span>
              <span v-else class="text-muted-foreground">未通过</span>
            </TableCell>
            <TableCell><Badge :variant="userStatusVariant(user.status)">{{ statusLabel[user.status] ?? user.status }}</Badge></TableCell>
            <TableCell class="hidden lg:table-cell">{{ formatTime(user.createdAt) }}</TableCell>
            <TableCell class="text-right"><AdminRowActions :actions="rowActions(user)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="8" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" empty-text="还没有账号"
            @retry="load" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="人" @change="list.setPage" />

  </div>
</template>
