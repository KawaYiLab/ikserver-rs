<script setup lang="ts">
import { ExternalLink, FileSearch, Undo2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { userPath } from '~/lib/links'
import { actionLabel, formatTime, reasonLabel, reportStatusLabel, reportStatusVariant, targetTypeLabel } from '~/lib/format'

type Group = Schemas['ReportGroup']
type Tab = 'pending' | 'reviewing' | 'closed'
type Action = Schemas['Action']

const { me } = useSession()
const siteLink = useSiteLink()
const { run, failure } = useNotify()
const badges = useBadges()

/** 结束日期包含当天：取到次日 00:00（北京时间）。 */
function dayBound(date: string, next = false) {
  if (!date) return undefined
  const at = new Date(`${date}T00:00:00+08:00`)
  if (Number.isNaN(at.getTime())) return undefined
  if (next) at.setDate(at.getDate() + 1)
  return at.toISOString()
}

const list = useListQuery({
  filters: { tab: 'pending', targetType: 'all', reason: 'all', q: '', from: '', to: '', mine: 'yes' },
  text: ['q'],
  persistent: ['tab', 'mine'],
  fetch: (f, page) => unwrap(api.GET('/v1/admin/reports', {
    params: {
      query: {
        tab: f.tab as Tab,
        targetType: optional<Schemas['TargetType']>(f.targetType),
        reason: optional<Schemas['Reason']>(f.reason),
        q: optional(f.q),
        from: dayBound(f.from),
        to: dayBound(f.to, true),
        mine: f.tab === 'reviewing' ? f.mine === 'yes' : undefined,
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading } = list
const tab = computed(() => list.applied.value.filters.tab)

function load() {
  list.refresh()
  badges.refresh()
}

function targetName(group: Group) {
  const t = group.target
  if (group.targetType === 'user') return `@${t.username ?? group.targetId}`
  if (group.targetType === 'post') return t.title?.trim() || '(无标题)'
  return t.postTitle ? `《${t.postTitle}》下的评论` : '评论'
}

function claimedByMe(group: Group) {
  return group.assignedTo?.id === me.value?.id
}

function siteUrl(group: Group) {
  if (!group.target.exists) return undefined
  if (group.targetType === 'post') return siteLink(`/post/${group.targetId}`)
  if (group.targetType === 'comment' && group.target.postId) return siteLink(`/post/${group.target.postId}?comment=${group.targetId}`)
  if (group.targetType === 'user') return siteLink(`/profile/${group.targetId}`)
  return undefined
}

function rowActions(group: Group): RowAction[] {
  const url = siteUrl(group)
  return [
    { label: '查看举报明细', icon: FileSearch, onSelect: () => openDetail(group) },
    { label: '在主站打开', icon: ExternalLink, href: url, hidden: !url },
    { label: '释放（退回待处理）', icon: Undo2, hidden: !(group.status === 'reviewing' && claimedByMe(group)), onSelect: () => unclaim(group) },
  ]
}

// ---------- 详情 ----------
const detail = ref<Group | null>(null)
const detailOpen = ref(false)

async function openDetail(group: Group) {
  detailOpen.value = true
  detail.value = group
  try {
    detail.value = await unwrap(api.GET('/v1/admin/reports/{targetType}/{targetId}', {
      params: { path: { targetType: group.targetType, targetId: group.targetId } },
    }))
  }
  catch (error) {
    failure(error)
  }
}

async function claim(group: Group) {
  const done = await run(() => unwrap(api.POST('/v1/admin/reports/{targetType}/{targetId}/claim', {
    params: { path: { targetType: group.targetType, targetId: group.targetId } },
  })), '已领取')
  if (done) {
    detailOpen.value = false
    load()
  }
}

async function unclaim(group: Group) {
  const done = await run(() => unwrap(api.POST('/v1/admin/reports/{targetType}/{targetId}/unclaim', {
    params: { path: { targetType: group.targetType, targetId: group.targetId } },
  })), '已释放')
  if (done) {
    detailOpen.value = false
    load()
  }
}

// ---------- 结案 ----------
const resolving = ref<Group | null>(null)
const action = ref<Action>('reject')
const note = ref('')
const targetNote = ref('')
const confirmText = ref('')
const submitting = ref(false)
const highRisk = computed(() => action.value === 'delete_content' || action.value === 'block_user')
const actions = computed<Action[]>(() =>
  resolving.value?.targetType === 'user'
    ? ['reject', 'warn_user', 'block_user']
    : ['reject', 'hide_content', 'delete_content', 'warn_user', 'block_user'],
)

function openResolve(group: Group) {
  resolving.value = group
  action.value = 'reject'
  note.value = ''
  targetNote.value = ''
  confirmText.value = ''
}

async function resolve() {
  const group = resolving.value
  if (!group) return
  submitting.value = true
  const done = await run(() => unwrap(api.POST('/v1/admin/reports/{targetType}/{targetId}/resolve', {
    params: { path: { targetType: group.targetType, targetId: group.targetId } },
    body: { action: action.value, note: note.value.trim() || null, targetNote: targetNote.value.trim() || null },
  })), `已${actionLabel[action.value]}`)
  submitting.value = false
  if (done) {
    resolving.value = null
    detailOpen.value = false
    load()
  }
}
</script>

<template>
  <div>
    <AdminPageHeader title="举报处理" description="同一目标的举报会一并结案。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <Tabs v-model="filters.tab" class="mb-4">
      <TabsList>
        <TabsTrigger value="pending">
          待处理<template v-if="tab === 'pending' && result"> ({{ result.total }})</template>
        </TabsTrigger>
        <TabsTrigger value="reviewing">
          复核中<template v-if="tab === 'reviewing' && result"> ({{ result.total }})</template>
        </TabsTrigger>
        <TabsTrigger value="closed">
          已结案<template v-if="tab === 'closed' && result"> ({{ result.total }})</template>
        </TabsTrigger>
      </TabsList>
    </Tabs>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-6">
      <Select v-model="filters.targetType">
        <SelectTrigger class="w-full"><SelectValue placeholder="目标类型" /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部类型</SelectItem>
          <SelectItem v-for="(label, key) in targetTypeLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.reason">
        <SelectTrigger class="w-full"><SelectValue placeholder="举报理由" /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部理由</SelectItem>
          <SelectItem v-for="(label, key) in reasonLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Input v-model="filters.q" placeholder="目标标题 / 举报人 / 被举报人" class="lg:col-span-2" />
      <Input v-model="filters.from" type="date" aria-label="起始日期" />
      <Input v-model="filters.to" type="date" aria-label="结束日期" />
    </div>
    <div v-if="tab === 'reviewing'" class="mb-4 flex gap-2">
      <Button size="sm" :variant="filters.mine === 'yes' ? 'default' : 'outline'" @click="filters.mine = 'yes'">只看我的</Button>
      <Button size="sm" :variant="filters.mine === 'yes' ? 'outline' : 'default'" @click="filters.mine = 'no'">查看全部</Button>
    </div>

    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>目标</TableHead>
            <TableHead class="hidden md:table-cell">类型</TableHead>
            <TableHead class="text-right">举报数</TableHead>
            <TableHead class="hidden md:table-cell">最新举报</TableHead>
            <TableHead>状态</TableHead>
            <TableHead class="hidden lg:table-cell">处理人</TableHead>
            <TableHead class="text-right">操作</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="group in result?.items ?? []" :key="`${group.targetType}:${group.targetId}`">
            <TableCell class="max-w-72">
              <button class="truncate text-left font-medium hover:underline" @click="openDetail(group)">
                {{ targetName(group) }}
              </button>
              <div class="mt-1 flex flex-wrap gap-1">
                <Badge v-if="group.target.hidden" variant="secondary">已隐藏</Badge>
                <Badge v-if="group.target.ownerBlocked" variant="secondary">已封禁</Badge>
                <Badge v-if="!group.target.exists" variant="outline">已删除</Badge>
              </div>
            </TableCell>
            <TableCell class="hidden md:table-cell">{{ targetTypeLabel[group.targetType] }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ group.reportCount }}</TableCell>
            <TableCell class="hidden md:table-cell">{{ formatTime(group.latestReportAt) }}</TableCell>
            <TableCell><Badge :variant="reportStatusVariant(group.status)">{{ reportStatusLabel[group.status] }}</Badge></TableCell>
            <TableCell class="hidden lg:table-cell">
              <template v-if="group.assignedTo">
                {{ group.assignedTo.username }}<span v-if="claimedByMe(group)">（我）</span>
              </template>
              <span v-else class="text-muted-foreground">—</span>
            </TableCell>
            <TableCell class="text-right whitespace-nowrap">
              <div class="flex items-center justify-end gap-1">
                <template v-if="tab !== 'closed'">
                  <Button v-if="group.status === 'pending'" size="sm" variant="outline" @click="claim(group)">领取</Button>
                  <Button v-if="group.status === 'pending' || claimedByMe(group)" size="sm" @click="openResolve(group)">处理</Button>
                  <span v-if="group.status === 'reviewing' && !claimedByMe(group)" class="text-xs text-muted-foreground">已被领取</span>
                </template>
                <AdminRowActions :actions="rowActions(group)" />
              </div>
            </TableCell>
          </TableRow>
          <AdminListState
            :colspan="7" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" :empty-text="tab === 'pending' ? '没有待处理的举报，都处理完了' : '没有举报'"
            @retry="load" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="个目标" @change="list.setPage" />

    <!-- 详情 -->
    <Sheet v-model:open="detailOpen">
      <SheetContent class="w-full overflow-y-auto sm:max-w-xl">
        <template v-if="detail">
          <SheetHeader>
            <div class="flex flex-wrap gap-2">
              <Badge variant="outline">{{ targetTypeLabel[detail.targetType] }}</Badge>
              <Badge :variant="reportStatusVariant(detail.status)">{{ reportStatusLabel[detail.status] }}</Badge>
              <Badge v-if="detail.reportCount > 1" variant="secondary">{{ detail.reportCount }} 人举报</Badge>
              <Badge v-if="detail.target.anonymous" variant="secondary">匿名</Badge>
            </div>
            <SheetTitle>{{ targetName(detail) }}</SheetTitle>
            <SheetDescription class="whitespace-pre-wrap">{{ detail.target.excerpt || '（没有摘要）' }}</SheetDescription>
          </SheetHeader>
          <div class="space-y-4 px-4 pb-6">
            <div v-if="detail.target.images.length" class="grid grid-cols-3 gap-2">
              <a v-for="image in detail.target.images" :key="image.id" :href="image.url" target="_blank" rel="noopener">
                <img :src="image.thumbUrl" :alt="image.name" class="aspect-square w-full rounded-md object-cover">
              </a>
            </div>
            <div class="flex flex-wrap gap-3 text-sm">
              <a v-if="detail.targetType === 'post' && detail.target.exists" :href="siteLink(`/post/${detail.targetId}`)" target="_blank" class="underline">
                查看帖子<template v-if="detail.target.hidden">（已隐藏）</template>
              </a>
              <a v-if="detail.target.postId" :href="siteLink(`/post/${detail.target.postId}?comment=${detail.targetId}`)" target="_blank" class="underline">查看评论</a>
              <a v-if="detail.targetType === 'user'" :href="siteLink(`/profile/${detail.targetId}`)" target="_blank" class="underline">查看主页</a>
              <NuxtLink
                v-if="detail.targetType === 'post' && detail.target.exists"
                :to="{ path: '/comments', query: { postId: detail.targetId } }" class="underline"
              >
                该帖全部评论
              </NuxtLink>
              <NuxtLink
                v-if="detail.targetType === 'comment' && detail.target.postId"
                :to="{ path: '/comments', query: { postId: detail.target.postId } }" class="underline"
              >
                同帖其他评论
              </NuxtLink>
              <span v-if="detail.target.owner">
                作者：<NuxtLink :to="userPath(detail.target.owner.id)" class="underline">@{{ detail.target.owner.username }}</NuxtLink>
              </span>
            </div>
            <div>
              <h3 class="mb-2 text-sm font-medium">举报明细</h3>
              <ul class="space-y-3">
                <li v-for="report in detail.reports" :key="report.id" class="rounded-md border p-3 text-sm">
                  <div class="flex flex-wrap justify-between gap-2">
                    <span class="font-medium">{{ reasonLabel[report.reason] }}</span>
                    <span class="text-muted-foreground">{{ formatTime(report.createdAt) }}</span>
                  </div>
                  <div class="mt-1 text-muted-foreground">
                    举报人：{{ report.reporter ? (report.reporter.status === 'deleted' ? '(已注销)' : `@${report.reporter.username}`) : '—' }}
                  </div>
                  <p v-if="report.detail" class="mt-1 whitespace-pre-wrap">{{ report.detail }}</p>
                  <div v-if="report.resolution" class="mt-2 space-y-1 border-t pt-2 text-muted-foreground">
                    <div>结果：{{ report.resolution }} · {{ report.handledBy?.username ?? '系统' }} · {{ formatTime(report.handledAt) }}</div>
                    <div v-if="report.resolutionNote">举报人备注：{{ report.resolutionNote }}</div>
                    <div v-if="report.targetResolutionNote">被举报人备注：{{ report.targetResolutionNote }}</div>
                  </div>
                </li>
              </ul>
            </div>
            <div v-if="detail.status === 'pending' || detail.status === 'reviewing'" class="flex flex-wrap items-center gap-2 border-t pt-4">
              <span v-if="detail.assignedTo" class="text-sm text-muted-foreground">处理人：{{ detail.assignedTo.username }}</span>
              <div class="flex-1" />
              <template v-if="detail.status === 'reviewing' && !claimedByMe(detail)">
                <span class="text-sm text-muted-foreground">该举报已被他人领取，仅处理人可结案</span>
              </template>
              <template v-else>
                <Button v-if="detail.status === 'pending'" variant="outline" @click="claim(detail)">领取</Button>
                <Button v-if="detail.status === 'reviewing'" variant="ghost" @click="unclaim(detail)">释放</Button>
                <Button @click="openResolve(detail)">处理</Button>
              </template>
            </div>
          </div>
        </template>
      </SheetContent>
    </Sheet>

    <!-- 结案 -->
    <Dialog :open="!!resolving" @update:open="(v) => { if (!v) resolving = null }">
      <DialogContent v-if="resolving">
        <DialogHeader>
          <DialogTitle>处理举报 — {{ targetName(resolving) }}</DialogTitle>
          <DialogDescription>同一目标的 {{ resolving.reportCount }} 条举报会一并结案。</DialogDescription>
        </DialogHeader>
        <div class="space-y-4">
          <div class="space-y-2">
            <Label>动作</Label>
            <Select v-model="action">
              <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem v-for="a in actions" :key="a" :value="a">
                  {{ actionLabel[a] }}<template v-if="a === 'delete_content' || a === 'block_user'">（高危）</template>
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">处理备注（给举报人，选填）</span>
            <Textarea v-model="note" maxlength="500" />
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">被举报人备注（选填）</span>
            <Textarea v-model="targetNote" maxlength="500" />
          </label>
          <div v-if="highRisk" class="space-y-2">
            <Label>这是高危操作，请输入「确认」</Label>
            <Input v-model="confirmText" placeholder="确认" />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="resolving = null">取消</Button>
          <Button
            :variant="highRisk ? 'destructive' : 'default'"
            :disabled="submitting || (highRisk && confirmText.trim() !== '确认')"
            @click="resolve"
          >
            确认{{ actionLabel[action] }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
