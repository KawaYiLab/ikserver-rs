<script setup lang="ts">
import type { Component } from 'vue'
import { Bot, CheckCircle2, ChevronRight, ClipboardList, Flag, Hand, MessagesSquare, Newspaper, SearchX, Users } from '@lucide/vue'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { auditActionLabel } from '~/lib/audit'
import { formatNumber, formatTime } from '~/lib/format'
import { cn } from '@/lib/utils'

const { isAdmin } = useSession()
const { failure } = useNotify()
const badges = useBadges()

const range = useQueryParam<'7' | '30'>('range', '7')
const metric = useQueryParam<'users' | 'posts' | 'comments' | 'reports'>('metric', 'posts')

const data = ref<Schemas['Dashboard'] | null>(null)
const trends = ref<Schemas['Trends'] | null>(null)
const search = ref<Schemas['SearchStatus'] | null>(null)
const audit = ref<Schemas['AuditEntry'][] | null>(null)
const claimed = ref<Schemas['ReportGroup'][] | null>(null)
const loading = ref(false)

async function loadTrends() {
  trends.value = await unwrap(api.GET('/v1/admin/dashboard/trends', { params: { query: { days: Number(range.value) } } }))
}

async function load() {
  loading.value = true
  try {
    const [dashboard] = await Promise.all([
      unwrap(api.GET('/v1/admin/dashboard')),
      loadTrends(),
      unwrap(api.GET('/v1/admin/reports', { params: { query: { tab: 'reviewing', mine: true, limit: 5 } } }))
        .then((page) => { claimed.value = page.items }),
      isAdmin.value
        ? unwrap(api.GET('/v1/admin/search')).then((s) => { search.value = s })
        : Promise.resolve(),
      isAdmin.value
        ? unwrap(api.GET('/v1/admin/audit', { params: { query: { limit: 8 } } })).then((page) => { audit.value = page.items })
        : Promise.resolve(),
    ])
    data.value = dashboard
    badges.dashboard.value = dashboard
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)
watch(range, () => loadTrends().catch(failure))

// ---------- 待办 ----------
interface Todo {
  key: string
  icon: Component
  title: string
  detail: string
  to: string | { path: string, query: Record<string, string> }
  tone: 'danger' | 'warning' | 'default'
}

const todos = computed<Todo[]>(() => {
  const d = data.value
  if (!d) return []
  const list: Todo[] = []
  if (d.reports.mine > 0) {
    list.push({ key: 'mine', icon: Hand, title: `我领取的举报 ${d.reports.mine} 个`, detail: '领取后只有你能结案，处理完或释放给别人', to: { path: '/reports', query: { tab: 'reviewing' } }, tone: 'warning' })
  }
  if (d.reports.pending > 0) {
    list.push({ key: 'pending', icon: Flag, title: `待处理举报 ${d.reports.pending} 个`, detail: `另有 ${d.reports.reviewing} 个复核中`, to: '/reports', tone: 'danger' })
  }
  if (d.exam.enabled < d.exam.questionCount) {
    list.push({ key: 'exam', icon: ClipboardList, title: '入站题库题目不够', detail: `启用 ${d.exam.enabled} 题，每场考试要抽 ${d.exam.questionCount} 题，新用户无法开考`, to: '/exam', tone: 'danger' })
  }
  if (isAdmin.value && d.ai.failedLast24h > 0) {
    list.push({ key: 'ai', icon: Bot, title: `AI 失败任务 ${d.ai.failedLast24h} 个（24 小时）`, detail: '查看失败原因', to: { path: '/ai', query: { tab: 'jobs', jobStatus: 'failed' } }, tone: 'warning' })
  }
  const s = search.value
  if (isAdmin.value && s?.engineConfigured && (s.engineError || s.indexedPosts == null)) {
    list.push({ key: 'search', icon: SearchX, title: '搜索引擎读不到索引统计', detail: s.engineError ?? '搜索可能正在走数据库兜底', to: '/search', tone: 'warning' })
  }
  return list
})

const toneClass: Record<Todo['tone'], string> = {
  danger: 'bg-destructive/10 text-destructive',
  warning: 'bg-warning/15 text-warning',
  default: 'bg-muted text-muted-foreground',
}

// ---------- 趋势 ----------
const metrics = [
  { key: 'users', label: '新注册' },
  { key: 'posts', label: '新委托' },
  { key: 'comments', label: '新评论' },
  { key: 'reports', label: '收到举报' },
] as const

function series(key: typeof metrics[number]['key']) {
  return trends.value?.[key] ?? []
}

function change(values: number[]) {
  const today = values[values.length - 1] ?? 0
  const yesterday = values[values.length - 2] ?? 0
  const diff = today - yesterday
  if (diff === 0) return '与昨天持平'
  return `较昨天 ${diff > 0 ? '+' : ''}${formatNumber(diff)}`
}

const activeMetric = computed(() => metrics.find(m => m.key === metric.value) ?? metrics[1])
const aiPercent = computed(() => {
  const ai = data.value?.ai
  if (!ai || ai.siteLimit <= 0) return null
  return Math.min(100, Math.round((ai.siteUsedToday / ai.siteLimit) * 100))
})
</script>

<template>
  <div>
    <AdminPageHeader title="工作台" description="按北京时间业务日（04:00 切日）统计；今天还没过完。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div v-if="!data" class="space-y-6">
      <Skeleton class="h-24" />
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <Skeleton v-for="i in 4" :key="i" class="h-32" />
      </div>
      <Skeleton class="h-72" />
    </div>

    <div v-else class="space-y-6">
      <!-- 待办 -->
      <section aria-labelledby="todo-title">
        <h2 id="todo-title" class="mb-3 text-sm font-medium text-muted-foreground">待办</h2>
        <div v-if="todos.length" class="grid gap-3 md:grid-cols-2">
          <NuxtLink
            v-for="todo in todos" :key="todo.key" :to="todo.to"
            class="group flex items-center gap-3 rounded-xl border bg-card p-4 shadow-xs transition-colors hover:bg-accent/60"
          >
            <span :class="cn('grid size-9 shrink-0 place-items-center rounded-lg', toneClass[todo.tone])">
              <component :is="todo.icon" class="size-4.5" />
            </span>
            <div class="min-w-0 flex-1">
              <div class="font-medium">{{ todo.title }}</div>
              <div class="truncate text-xs text-muted-foreground">{{ todo.detail }}</div>
            </div>
            <ChevronRight class="size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
          </NuxtLink>
        </div>
        <div v-else class="flex items-center gap-3 rounded-xl border bg-card p-4 text-sm text-muted-foreground shadow-xs">
          <span class="grid size-9 place-items-center rounded-lg bg-success/10 text-success"><CheckCircle2 class="size-4.5" /></span>都处理完了，没有待办。
        </div>
      </section>

      <!-- 趋势 -->
      <section aria-labelledby="trend-title" class="space-y-4">
        <div class="flex items-center justify-between gap-2">
          <h2 id="trend-title" class="text-sm font-medium text-muted-foreground">趋势</h2>
          <Tabs v-model="range">
            <TabsList>
              <TabsTrigger value="7">7 天</TabsTrigger>
              <TabsTrigger value="30">30 天</TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <InspiraCardSpotlight
            v-for="m in metrics" :key="m.key" as="button" type="button" :aria-pressed="metric === m.key"
            :class="cn(
              'p-4 text-left shadow-xs transition-[border-color,box-shadow]',
              metric === m.key ? 'border-brand-strong shadow-[inset_0_-3px_0_var(--brand-strong)]' : 'hover:border-foreground/20',
            )"
            @click="metric = m.key"
          >
            <div class="text-sm text-muted-foreground">{{ m.label }} · 今天</div>
            <div class="mt-1 text-2xl font-semibold tracking-tight">
              <InspiraNumberTicker :value="series(m.key).at(-1) ?? 0" />
            </div>
            <div class="text-xs text-muted-foreground">{{ change(series(m.key)) }}</div>
            <AdminSparkline class="mt-2" :values="series(m.key)" />
          </InspiraCardSpotlight>
        </div>
        <Card class="shadow-xs">
          <CardHeader class="pb-0">
            <CardTitle class="text-base">{{ activeMetric.label }} · 最近 {{ range }} 天</CardTitle>
          </CardHeader>
          <CardContent>
            <AdminTrendChart v-if="trends" :days="trends.days" :values="series(activeMetric.key)" :label="activeMetric.label" />
          </CardContent>
        </Card>
      </section>

      <div class="grid gap-6 lg:grid-cols-2">
        <!-- 总量 -->
        <section aria-labelledby="total-title">
          <h2 id="total-title" class="mb-3 text-sm font-medium text-muted-foreground">总量</h2>
          <div class="grid grid-cols-2 gap-4">
            <NuxtLink to="/users" class="block"><AdminStatCard title="账号" :icon="Users" :value="data.users.total" :hint="`封禁中 ${formatNumber(data.users.blocked)}`" /></NuxtLink>
            <NuxtLink to="/posts" class="block"><AdminStatCard title="委托" :icon="Newspaper" :value="data.posts.total" :hint="`今日 ${formatNumber(data.posts.today)}`" /></NuxtLink>
            <NuxtLink to="/comments" class="block"><AdminStatCard title="评论" :icon="MessagesSquare" :value="data.comments.total" :hint="`今日 ${formatNumber(data.comments.today)}`" /></NuxtLink>
            <AdminStatCard
              title="AI 助手" :icon="Bot" :value="data.ai.globalEnabled ? '已开启' : '已关闭'" :tone="data.ai.globalEnabled ? 'default' : 'warning'"
              :hint="aiPercent === null ? `今日全站用量 ${formatNumber(data.ai.siteUsedToday)}（不限）` : `今日全站额度已用 ${aiPercent}%`"
            />
          </div>
        </section>

        <!-- 我领取的 / 最近操作 -->
        <section aria-labelledby="recent-title">
          <h2 id="recent-title" class="mb-3 text-sm font-medium text-muted-foreground">{{ isAdmin ? '最近的后台操作' : '我领取的举报' }}</h2>
          <div class="overflow-hidden rounded-xl border bg-card shadow-xs">
            <template v-if="isAdmin">
              <ul v-if="audit?.length" class="divide-y">
                <li v-for="entry in audit" :key="entry.id" class="flex items-center gap-3 px-4 py-2.5 text-sm">
                  <span class="min-w-0 flex-1 truncate">
                    <span class="font-medium">{{ entry.actorName ? `@${entry.actorName}` : '(已删除)' }}</span>
                    {{ auditActionLabel[entry.action] ?? entry.action }}
                    <span v-if="entry.targetId" class="font-mono text-xs text-muted-foreground">{{ entry.targetId }}</span>
                  </span>
                  <span class="shrink-0 text-xs text-muted-foreground">{{ formatTime(entry.createdAt) }}</span>
                </li>
              </ul>
              <p v-else class="p-6 text-center text-sm text-muted-foreground">还没有后台操作</p>
              <NuxtLink to="/audit" class="block border-t px-4 py-2 text-center text-xs text-muted-foreground hover:text-foreground">查看全部审计日志</NuxtLink>
            </template>
            <template v-else>
              <ul v-if="claimed?.length" class="divide-y">
                <li v-for="group in claimed" :key="`${group.targetType}:${group.targetId}`" class="flex items-center gap-3 px-4 py-2.5 text-sm">
                  <span class="min-w-0 flex-1 truncate">{{ group.target.title ?? group.target.postTitle ?? group.target.username ?? group.targetId }}</span>
                  <span class="shrink-0 text-xs text-muted-foreground">{{ group.reportCount }} 条 · {{ formatTime(group.latestReportAt) }}</span>
                </li>
              </ul>
              <p v-else class="p-6 text-center text-sm text-muted-foreground">没有领取中的举报</p>
              <NuxtLink :to="{ path: '/reports', query: { tab: 'reviewing' } }" class="block border-t px-4 py-2 text-center text-xs text-muted-foreground hover:text-foreground">去举报处理</NuxtLink>
            </template>
          </div>
        </section>
      </div>
    </div>
  </div>
</template>
