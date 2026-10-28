<script setup lang="ts">
import { Clock, Database, HardDrive, ListChecks, Mail, RotateCcw, Search, Server, Skull, Trash2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime } from '~/lib/format'

definePageMeta({ admin: true })

type JobState = 'failed' | 'scheduled' | 'running'

const { run, failure, success } = useNotify()
const confirm = useConfirm()
const { me } = useSession()
const status = ref<Schemas['SystemStatus'] | null>(null)
const jobs = ref<Schemas['QueuedJob'][]>([])
const jobState = ref<JobState>('failed')
const loading = ref(false)
const jobsLoading = ref(false)
const jobsFailed = ref(false)

async function loadStatus() {
  try {
    status.value = await unwrap(api.GET('/v1/admin/system'))
  }
  catch (error) {
    failure(error)
  }
}
async function loadJobs() {
  jobsLoading.value = true
  jobsFailed.value = false
  try {
    jobs.value = await unwrap(api.GET('/v1/admin/system/jobs', { params: { query: { state: jobState.value } } }))
  }
  catch (error) {
    jobsFailed.value = true
    failure(error)
  }
  finally {
    jobsLoading.value = false
  }
}
async function load() {
  loading.value = true
  await Promise.all([loadStatus(), loadJobs()])
  loading.value = false
}
onMounted(load)
watch(jobState, loadJobs)

// 运行时长每分钟走一次。
const now = ref(Date.now())
const ticker = setInterval(() => (now.value = Date.now()), 60_000)
onBeforeUnmount(() => clearInterval(ticker))
const uptime = computed(() => {
  if (!status.value) return '—'
  const minutes = Math.max(0, Math.floor((now.value - new Date(status.value.startedAt).getTime()) / 60_000))
  const days = Math.floor(minutes / 1440)
  const hours = Math.floor((minutes % 1440) / 60)
  if (days) return `${days} 天 ${hours} 小时`
  if (hours) return `${hours} 小时 ${minutes % 60} 分`
  return `${minutes} 分钟`
})

function bytes(value?: number | null) {
  if (value == null) return '—'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let n = value
  let i = 0
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024
    i++
  }
  return `${n.toFixed(i ? 1 : 0)} ${units[i]}`
}

/** 任务类型的中文名；没收录的原样显示。 */
const taskLabel: Record<string, string> = {
  'content.fanout_new_post': '新帖通知关注者',
  'media.sweep_staging': '清理未完成的上传',
  'mihoyo.prune_qr_logins': '清理过期扫码登录',
  'server.prune_idempotency_keys': '清理幂等键',
  'ai.comment_reply': 'AI 回复评论',
  'ai.dm_reply': 'AI 回复私信',
  'ai.summarize_session': 'AI 会话摘要',
  'ai.extract_memories': 'AI 提取记忆',
  'ai.consolidate_memories': 'AI 整理记忆',
  'ai.nightly_memory': 'AI 夜间记忆整理',
  'ai.reclaim_quota_holds': '回收 AI 额度预占',
  'ai.refresh_mcp_tools': '刷新 MCP 工具',
  'qq.reply': 'QQ 回复',
  'qq.dispatch': 'QQ 投递',
  'qq.reconcile': 'QQ 对账',
  'qq.prune': 'QQ 清理',
  'qq.extract_memories': 'QQ 提取记忆',
  'qq.extract_group_memories': 'QQ 提取群记忆',
  'qq.recover_memories': 'QQ 补提记忆',
}

interface Component {
  key: string
  label: string
  icon: typeof Database
  tone: 'ok' | 'warn' | 'error'
  state: string
  details: string[]
}
const components = computed<Component[]>(() => {
  const s = status.value
  if (!s) return []
  const db = s.database
  const search = s.search
  const mail = s.mail
  return [
    {
      key: 'database',
      label: '数据库',
      icon: Database,
      tone: !db.ok ? 'error' : db.pendingMigrations ? 'warn' : 'ok',
      state: !db.ok ? '不可用' : db.pendingMigrations ? `${db.pendingMigrations} 个迁移未应用` : '正常',
      details: db.ok
        ? [
            `PostgreSQL ${db.serverVersion ?? '—'}`,
            `延迟 ${db.latencyMs?.toFixed(1) ?? '—'} ms`,
            `占用 ${bytes(db.sizeBytes)}`,
            `连接 ${db.poolSize - db.poolIdle}/${db.poolSize} 在用`,
            `迁移 ${db.migration ?? '—'}`,
          ]
        : [],
    },
    {
      key: 'search',
      label: '搜索引擎',
      icon: Search,
      tone: !search.configured ? 'warn' : search.ok ? 'ok' : 'error',
      state: !search.configured ? '未配置，走数据库搜索' : search.ok ? '正常' : '不可用',
      details: search.configured && search.ok
        ? [`帖子 ${formatNumber(search.posts)}`, `评论 ${formatNumber(search.comments)}`]
        : [],
    },
    {
      key: 'storage',
      label: '对象存储',
      icon: HardDrive,
      tone: s.storage.configured ? 'ok' : 'warn',
      state: s.storage.configured ? '已配置' : '未配置，上传不可用',
      details: s.storage.publicBaseUrl ? [s.storage.publicBaseUrl] : [],
    },
    {
      key: 'mail',
      label: '发信',
      icon: Mail,
      tone: mail.provider === 'dev_log' ? 'warn' : 'ok',
      state: mail.provider === 'dev_log' ? '开发模式，只写日志不发信' : '双通道',
      details: mail.provider === 'dev_log'
        ? []
        : [`SMTP ${mail.smtp ? '已配置' : '未配置'}`, `Brevo ${mail.brevo ? '已配置' : '未配置'}`, mail.fallback ? '失败时改走另一通道' : '不改走备用通道'],
    },
  ]
})
const toneDot = { ok: 'bg-success', warn: 'bg-warning', error: 'bg-destructive' }

const mailTo = ref('')
watch(me, value => (mailTo.value ||= value?.email ?? ''), { immediate: true })
const sending = ref(false)
async function sendTestMail() {
  sending.value = true
  try {
    const result = await unwrap(api.POST('/v1/admin/system/test-mail', { body: { to: mailTo.value } }))
    if (!result.ok) failure(new Error(`没有发出去：${result.error ?? '未知错误'}`))
    else success(result.devLog ? `开发模式：已写入日志，没有真的发给 ${result.to}` : `已发给 ${result.to}，请查收`)
  }
  catch (error) {
    failure(error)
  }
  finally {
    sending.value = false
  }
}

async function retry(job: Schemas['QueuedJob']) {
  const done = await run(() => unwrap(api.POST('/v1/admin/system/jobs/{id}/retry', { params: { path: { id: job.id } } })), '已重新排队')
  if (done) load()
}
async function remove(job: Schemas['QueuedJob']) {
  const ok = await confirm({
    title: '删除这个任务？',
    description: `「${taskLabel[job.task] ?? job.task}」将从队列移除，不再执行。`,
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/system/jobs/{id}', { params: { path: { id: job.id } } })).then(() => true), '已删除')
  if (done) load()
}
</script>

<template>
  <div>
    <AdminPageHeader title="系统状态" description="本实例的运行情况与各组件的配置。密钥不在这里显示，只说配没配。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
      <AdminStatCard title="版本" :value="status?.version ?? '—'" :hint="status ? `节点 ${status.nodeId}${status.cluster ? ' · 多实例' : ''}` : undefined" :icon="Server" />
      <AdminStatCard title="运行时长" :value="uptime" :hint="status ? `启动于 ${formatTime(status.startedAt)}` : undefined" :icon="Clock" />
      <AdminStatCard title="排队中的任务" :value="status ? status.jobs.ready + status.jobs.scheduled : '—'" :hint="status ? `${status.jobs.running} 个正在执行` : undefined" :icon="ListChecks" />
      <AdminStatCard title="死信任务" :value="status?.jobs.failed ?? '—'" hint="用完重试次数，不再执行" :icon="Skull" :tone="status?.jobs.failed ? 'danger' : 'default'" />
    </div>

    <Card class="mt-6 gap-0 py-0">
      <CardHeader class="border-b py-5">
        <CardTitle class="text-base">
          组件
        </CardTitle>
      </CardHeader>
      <CardContent class="divide-y py-0">
        <template v-if="status">
          <div v-for="item in components" :key="item.key" class="flex flex-col gap-3 py-4 sm:flex-row sm:items-center">
            <div class="flex min-w-0 flex-1 items-start gap-3">
              <div class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted">
                <component :is="item.icon" class="size-4 text-muted-foreground" />
              </div>
              <div class="min-w-0">
                <div class="flex items-center gap-2 text-sm font-medium">
                  {{ item.label }}
                  <span :class="cn('size-1.5 rounded-full', toneDot[item.tone])" />
                  <span class="font-normal text-muted-foreground">{{ item.state }}</span>
                </div>
                <p v-if="item.details.length" class="mt-0.5 text-xs break-all text-muted-foreground">
                  {{ item.details.join(' · ') }}
                </p>
              </div>
            </div>
            <form v-if="item.key === 'mail'" class="flex gap-2 sm:w-96" @submit.prevent="sendTestMail">
              <Input v-model="mailTo" type="email" placeholder="收件邮箱" aria-label="测试邮件收件人" required />
              <Button type="submit" variant="outline" class="shrink-0" :disabled="sending || !mailTo">
                {{ sending ? '发送中…' : '发测试邮件' }}
              </Button>
            </form>
          </div>
        </template>
        <div v-else class="space-y-3 py-4">
          <Skeleton v-for="i in 4" :key="i" class="h-10 w-full" />
        </div>
      </CardContent>
    </Card>

    <Card class="mt-6 gap-0 py-0">
      <CardHeader class="border-b py-5">
        <CardTitle class="text-base">
          后台任务
        </CardTitle>
        <CardDescription>失败的任务按指数退避自动重试；用完重试次数后成为死信，留在这里等你处理。</CardDescription>
      </CardHeader>
      <CardContent class="p-0">
        <Table v-if="status?.jobs.tasks.length">
          <TableHeader>
            <TableRow>
              <TableHead class="pl-6">
                任务
              </TableHead>
              <TableHead class="text-right">
                待执行
              </TableHead>
              <TableHead class="text-right">
                等待重试 / 定时
              </TableHead>
              <TableHead class="text-right">
                执行中
              </TableHead>
              <TableHead class="pr-6 text-right">
                死信
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="task in status.jobs.tasks" :key="task.task">
              <TableCell class="pl-6">
                <div>{{ taskLabel[task.task] ?? task.task }}</div>
                <div class="font-mono text-xs text-muted-foreground">
                  {{ task.task }}
                </div>
              </TableCell>
              <TableCell class="text-right tabular-nums">
                {{ formatNumber(task.ready) }}
              </TableCell>
              <TableCell class="text-right tabular-nums">
                {{ formatNumber(task.scheduled) }}
              </TableCell>
              <TableCell class="text-right tabular-nums">
                {{ formatNumber(task.running) }}
              </TableCell>
              <TableCell :class="cn('pr-6 text-right tabular-nums', task.failed && 'font-medium text-destructive')">
                {{ formatNumber(task.failed) }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
        <p v-else-if="status" class="px-6 py-8 text-center text-sm text-muted-foreground">
          队列是空的
        </p>

        <div class="border-t px-6 pt-4">
          <Tabs v-model="jobState">
            <TabsList>
              <TabsTrigger value="failed">
                死信
              </TabsTrigger>
              <TabsTrigger value="scheduled">
                等待中
              </TabsTrigger>
              <TabsTrigger value="running">
                执行中
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
        <Table class="mt-2">
          <TableHeader>
            <TableRow>
              <TableHead class="pl-6">
                任务
              </TableHead>
              <TableHead>尝试</TableHead>
              <TableHead>最后错误</TableHead>
              <TableHead>{{ jobState === 'scheduled' ? '下次执行' : '更新于' }}</TableHead>
              <TableHead class="w-24 pr-6" />
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="job in jobs" :key="job.id">
              <TableCell class="pl-6">
                <div>{{ taskLabel[job.task] ?? job.task }}</div>
                <div class="font-mono text-xs text-muted-foreground">
                  #{{ job.id }}
                </div>
              </TableCell>
              <TableCell class="tabular-nums">
                {{ job.attempts }}/{{ job.maxAttempts }}
              </TableCell>
              <TableCell class="max-w-md">
                <p v-if="job.lastError" class="line-clamp-2 font-mono text-xs break-all whitespace-normal text-muted-foreground" :title="job.lastError">
                  {{ job.lastError }}
                </p>
                <span v-else class="text-muted-foreground">—</span>
              </TableCell>
              <TableCell class="text-muted-foreground tabular-nums">
                {{ formatTime(jobState === 'scheduled' ? job.runAt : job.updatedAt) }}
              </TableCell>
              <TableCell class="pr-6">
                <div v-if="jobState !== 'running'" class="flex justify-end gap-1">
                  <Button variant="ghost" size="icon-sm" :aria-label="`立即重试 #${job.id}`" title="立即重试" @click="retry(job)">
                    <RotateCcw />
                  </Button>
                  <Button variant="ghost" size="icon-sm" class="text-muted-foreground" :aria-label="`删除 #${job.id}`" title="删除" @click="remove(job)">
                    <Trash2 />
                  </Button>
                </div>
                <Badge v-else variant="secondary">
                  执行中
                </Badge>
              </TableCell>
            </TableRow>
            <AdminListState
              :colspan="5" :loading="jobsLoading" :empty="!jobs.length" :failed="jobsFailed"
              :empty-text="{ failed: '没有死信任务', scheduled: '没有等待中的任务', running: '没有正在执行的任务' }[jobState]"
              :rows="3" @retry="loadJobs"
            />
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  </div>
</template>
