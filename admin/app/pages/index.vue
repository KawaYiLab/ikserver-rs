<script setup lang="ts">
import type { Component } from 'vue'
import { ArrowDownRight, ArrowRight, ArrowUpRight, Bot, CheckCircle2, ChevronRight, ClipboardList, Flag, Hand, MessagesSquare, Newspaper, SearchX, Users } from '@lucide/vue'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { auditActionLabel } from '~/lib/audit'
import { formatNumber, formatRelative } from '~/lib/format'
import { cn } from '@/lib/utils'

const { isAdmin, me } = useSession()
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
    list.push({ key: 'mine', icon: Hand, title: `我领取的举报 ${d.reports.mine} 个`, detail: '等你结案', to: { path: '/reports', query: { tab: 'reviewing' } }, tone: 'warning' })
  }
  if (d.reports.pending > 0) {
    list.push({ key: 'pending', icon: Flag, title: `待处理举报 ${d.reports.pending} 个`, detail: `另有 ${d.reports.reviewing} 个复核中`, to: '/reports', tone: 'danger' })
  }
  if (d.exam.enabled < d.exam.questionCount) {
    list.push({ key: 'exam', icon: ClipboardList, title: '入站题库题目不够', detail: `启用 ${d.exam.enabled} 题，每场需要 ${d.exam.questionCount} 题`, to: '/exam', tone: 'danger' })
  }
  if (isAdmin.value && d.ai.failedLast24h > 0) {
    list.push({ key: 'ai', icon: Bot, title: `AI 失败任务 ${d.ai.failedLast24h} 个（24 小时）`, detail: '查看失败原因', to: { path: '/ai', query: { tab: 'jobs', jobStatus: 'failed' } }, tone: 'warning' })
  }
  const s = search.value
  if (isAdmin.value && s?.engineConfigured && (s.engineError || s.indexedPosts == null)) {
    list.push({ key: 'search', icon: SearchX, title: '搜索引擎读不到索引统计', detail: s.engineError ?? '搜索可能已切换到数据库', to: '/search', tone: 'warning' })
  }
  return list
})

const toneClass: Record<Todo['tone'], string> = {
  danger: 'bg-destructive/10 text-destructive',
  warning: 'bg-warning/15 text-warning',
  default: 'bg-muted text-muted-foreground',
}

// ---------- 趋势 ----------
// `upIsGood`：举报变多是坏事，涨跌颜色反过来。
const metrics = [
  { key: 'users', label: '新注册', upIsGood: true },
  { key: 'posts', label: '新委托', upIsGood: true },
  { key: 'comments', label: '新评论', upIsGood: true },
  { key: 'reports', label: '收到举报', upIsGood: false },
] as const

function series(key: typeof metrics[number]['key']) {
  return trends.value?.[key] ?? []
}

function change(values: number[]) {
  const today = values[values.length - 1] ?? 0
  const yesterday = values[values.length - 2] ?? 0
  return today - yesterday
}
function deltaClass(diff: number, upIsGood: boolean) {
  if (diff === 0) return 'bg-muted text-muted-foreground'
  return (diff > 0) === upIsGood ? 'bg-success/10 text-success' : 'bg-destructive/10 text-destructive'
}

const greeting = computed(() => {
  const hour = Number(new Intl.DateTimeFormat('en-US', { hour: 'numeric', hourCycle: 'h23', timeZone: 'Asia/Shanghai' }).format(new Date()))
  const part = hour < 5 ? '夜深了' : hour < 11 ? '早上好' : hour < 13 ? '中午好' : hour < 18 ? '下午好' : '晚上好'
  return me.value ? `${part}，${me.value.username}` : '工作台'
})

const activeMetric = computed(() => metrics.find(m => m.key === metric.value) ?? metrics[1])
const aiPercent = computed(() => {
  const ai = data.value?.ai
  if (!ai || ai.siteLimit <= 0) return null
  return Math.min(100, Math.round((ai.siteUsedToday / ai.siteLimit) * 100))
})
</script>

<template>
  <div>
    <AdminPageHeader :title="greeting" description="数据按北京时间统计，每天 04:00 切日。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div v-if="!data" class="space-y-6">
      <Skeleton class="h-20 rounded-xl" />
      <Skeleton class="h-96 rounded-xl" />
      <div class="grid gap-6 lg:grid-cols-2">
        <Skeleton class="h-64 rounded-xl" />
        <Skeleton class="h-64 rounded-xl" />
      </div>
    </div>

    <div v-else class="space-y-8">
      <!-- 待办 -->
      <AdminReveal as="section" aria-labelledby="todo-title">
        <h2 id="todo-title" class="sr-only">待办</h2>
        <div v-if="todos.length" class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          <NuxtLink
            v-for="todo in todos" :key="todo.key" :to="todo.to"
            class="group flex items-center gap-3 rounded-xl border bg-card p-3.5 shadow-[var(--shadow-card)] transition-[border-color,background-color] hover:border-foreground/15 hover:bg-accent/40"
          >
            <span :class="cn('grid size-9 shrink-0 place-items-center rounded-lg', toneClass[todo.tone])">
              <component :is="todo.icon" class="size-4" />
            </span>
            <div class="min-w-0 flex-1">
              <div class="text-sm font-medium">{{ todo.title }}</div>
              <div class="truncate text-xs text-muted-foreground">{{ todo.detail }}</div>
            </div>
            <ChevronRight class="size-4 text-muted-foreground/70 transition-transform group-hover:translate-x-0.5" />
          </NuxtLink>
        </div>
        <div v-else class="flex items-center gap-3 rounded-xl border border-dashed p-3.5 text-sm text-muted-foreground">
          <span class="grid size-9 place-items-center rounded-lg bg-success/10 text-success"><CheckCircle2 class="size-4" /></span>都处理完了，没有待办。
        </div>
      </AdminReveal>

      <!-- 趋势：上面四个指标即标签页，下面是选中指标的柱状图 -->
      <AdminReveal :index="1" as="section" aria-labelledby="trend-title" class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
        <div class="flex items-center justify-between gap-2 border-b px-5 py-3">
          <h2 id="trend-title" class="text-sm font-medium">
            趋势
          </h2>
          <Tabs v-model="range">
            <TabsList class="h-8">
              <TabsTrigger value="7" class="text-xs">
                7 天
              </TabsTrigger>
              <TabsTrigger value="30" class="text-xs">
                30 天
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
        <div class="grid grid-cols-2 border-b lg:grid-cols-4" role="tablist" aria-label="指标">
          <button
            v-for="(m, i) in metrics" :key="m.key" type="button" role="tab" :aria-selected="metric === m.key"
            :class="cn(
              'relative px-5 py-4 text-left transition-colors hover:bg-muted/40',
              i % 2 === 1 && 'border-l', i >= 2 && 'border-t lg:border-t-0', i === 2 && 'lg:border-l',
              metric === m.key ? 'bg-muted/30' : '',
            )"
            @click="metric = m.key"
          >
            <Motion
              v-if="metric === m.key" layout-id="trend-metric" aria-hidden="true"
              class="absolute inset-x-0 top-0 h-0.5 bg-foreground"
              :transition="{ type: 'spring', stiffness: 500, damping: 40 }"
            />
            <div class="text-xs text-muted-foreground">
              {{ m.label }} · 今天
            </div>
            <div class="mt-1.5 flex items-baseline gap-2">
              <span class="text-2xl font-semibold tracking-tight tabular-nums">
                <InspiraNumberTicker :value="series(m.key).at(-1) ?? 0" />
              </span>
              <span :class="cn('inline-flex items-center gap-0.5 rounded-md px-1.5 py-0.5 text-[11px] font-medium tabular-nums', deltaClass(change(series(m.key)), m.upIsGood))">
                <ArrowUpRight v-if="change(series(m.key)) > 0" class="size-3" />
                <ArrowDownRight v-else-if="change(series(m.key)) < 0" class="size-3" />
                {{ change(series(m.key)) === 0 ? '持平' : formatNumber(Math.abs(change(series(m.key)))) }}
              </span>
            </div>
            <AdminSparkline :class="cn('mt-3 transition-colors', metric === m.key ? 'text-foreground' : 'text-muted-foreground/60')" :values="series(m.key)" />
          </button>
        </div>
        <div class="px-5 pt-5 pb-3">
          <div class="mb-2 text-xs text-muted-foreground">
            {{ activeMetric.label }} · 最近 {{ range }} 天
          </div>
          <AdminTrendChart v-if="trends" :days="trends.days" :values="series(activeMetric.key)" :label="activeMetric.label" />
        </div>
      </AdminReveal>

      <div class="grid gap-6 lg:grid-cols-5">
        <!-- 总量 -->
        <AdminReveal :index="2" as="section" aria-labelledby="total-title" class="lg:col-span-2">
          <h2 id="total-title" class="mb-3 text-sm font-medium">
            总量
          </h2>
          <div class="grid grid-cols-2 gap-3">
            <NuxtLink to="/users" class="block">
              <AdminStatCard title="账号" :icon="Users" :value="data.users.total" :hint="`封禁中 ${formatNumber(data.users.blocked)}`" />
            </NuxtLink>
            <NuxtLink to="/posts" class="block">
              <AdminStatCard title="委托" :icon="Newspaper" :value="data.posts.total" :hint="`今日 +${formatNumber(data.posts.today)}`" />
            </NuxtLink>
            <NuxtLink to="/comments" class="block">
              <AdminStatCard title="评论" :icon="MessagesSquare" :value="data.comments.total" :hint="`今日 +${formatNumber(data.comments.today)}`" />
            </NuxtLink>
            <AdminStatCard
              title="AI 助手" :icon="Bot" :value="data.ai.globalEnabled ? '已开启' : '已关闭'" :tone="data.ai.globalEnabled ? 'default' : 'warning'"
              :hint="aiPercent === null ? `今日全站用量 ${formatNumber(data.ai.siteUsedToday)}（不限）` : `今日额度已用 ${aiPercent}%`"
              :progress="aiPercent ?? undefined"
            />
          </div>
        </AdminReveal>

        <!-- 我领取的 / 最近操作 -->
        <AdminReveal :index="3" as="section" aria-labelledby="recent-title" class="lg:col-span-3">
          <div class="mb-3 flex items-center justify-between">
            <h2 id="recent-title" class="text-sm font-medium">
              {{ isAdmin ? '最近的后台操作' : '我领取的举报' }}
            </h2>
            <NuxtLink
              :to="isAdmin ? '/audit' : { path: '/reports', query: { tab: 'reviewing' } }"
              class="group inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
            >
              {{ isAdmin ? '全部审计日志' : '去举报处理' }}<ArrowRight class="size-3 transition-transform group-hover:translate-x-0.5" />
            </NuxtLink>
          </div>
          <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
            <template v-if="isAdmin">
              <ul v-if="audit?.length" class="divide-y">
                <li v-for="entry in audit" :key="entry.id" class="flex items-center gap-3 px-4 py-2.5 text-sm">
                  <span class="grid size-7 shrink-0 place-items-center rounded-full bg-muted text-[11px] font-semibold text-muted-foreground">
                    {{ (entry.actorName ?? '?').slice(0, 1).toUpperCase() }}
                  </span>
                  <span class="min-w-0 flex-1 truncate">
                    <span class="font-medium">{{ entry.actorName ?? '(已删除)' }}</span>
                    <span class="ml-1.5 text-muted-foreground">{{ auditActionLabel[entry.action] ?? entry.action }}</span>
                    <span v-if="entry.targetId" class="ml-1 rounded bg-muted px-1 font-mono text-[11px] text-muted-foreground">{{ entry.targetId }}</span>
                  </span>
                  <span class="shrink-0 text-xs text-muted-foreground tabular-nums">{{ formatRelative(entry.createdAt) }}</span>
                </li>
              </ul>
              <p v-else class="p-8 text-center text-sm text-muted-foreground">
                还没有后台操作
              </p>
            </template>
            <template v-else>
              <ul v-if="claimed?.length" class="divide-y">
                <li v-for="group in claimed" :key="`${group.targetType}:${group.targetId}`" class="flex items-center gap-3 px-4 py-2.5 text-sm">
                  <span class="min-w-0 flex-1 truncate">{{ group.target.title ?? group.target.postTitle ?? group.target.username ?? group.targetId }}</span>
                  <span class="shrink-0 text-xs text-muted-foreground">{{ group.reportCount }} 条 · {{ formatRelative(group.latestReportAt) }}</span>
                </li>
              </ul>
              <p v-else class="p-8 text-center text-sm text-muted-foreground">
                没有领取中的举报
              </p>
            </template>
          </div>
        </AdminReveal>
      </div>
    </div>
  </div>
</template>
