<script setup lang="ts">
import { watchDebounced } from '@vueuse/core'
import { RefreshCw } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime, roleLabel, statusLabel } from '~/lib/format'

type User = Schemas['AdminUser']
type Role = Schemas['Role']

const { me, isAdmin } = useSession()
const { run, failure, success } = useNotify()
const confirm = useConfirm()

const q = ref('')
const role = ref('all')
const status = ref('all')
const ai = ref('all')
const page = ref(1)
const limit = 20
const result = ref<Schemas['Paged_AdminUser'] | null>(null)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    result.value = await unwrap(api.GET('/v1/admin/users', {
      params: {
        query: {
          q: q.value.trim() || undefined,
          role: role.value === 'all' ? undefined : role.value,
          status: status.value === 'all' ? undefined : status.value,
          ai: ai.value === 'all' ? undefined : ai.value === 'yes',
          page: page.value,
          limit,
        },
      },
    }))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}

watch([role, status, ai], () => {
  page.value = 1
  load()
})
watchDebounced(q, () => {
  page.value = 1
  load()
}, { debounce: 400 })
onMounted(load)

function statusVariant(value: string) {
  return value === 'active' ? 'outline' : value === 'blocked' ? 'destructive' : 'secondary'
}

// ---------- 详情 ----------
const detail = ref<Schemas['AdminUserDetail'] | null>(null)
const open = ref(false)
const isSelf = computed(() => detail.value?.user.id === me.value?.id)
const editable = computed(() => detail.value?.user.status === 'active')

async function openDetail(user: User) {
  open.value = true
  detail.value = null
  resetForms()
  try {
    detail.value = await unwrap(api.GET('/v1/admin/users/{id}', { params: { path: { id: user.id } } }))
  }
  catch (error) {
    failure(error)
  }
}

async function refreshDetail() {
  const id = detail.value?.user.id
  if (!id) return
  detail.value = await unwrap(api.GET('/v1/admin/users/{id}', { params: { path: { id } } }))
  load()
}

async function changeRole(next: Role) {
  const user = detail.value?.user
  if (!user || next === user.role) return
  const ok = await confirm({
    title: `把 @${user.username} 设为${roleLabel[next]}？`,
    description: next === 'user' ? '对方将失去后台权限。' : '对方将可以登录管理后台。',
  })
  if (!ok) return
  const updated = await run(() => unwrap(api.PUT('/v1/admin/users/{id}/role', {
    params: { path: { id: user.id } },
    body: { role: next },
  })), '角色已更新')
  if (updated) {
    detail.value = updated
    load()
  }
}

async function changeAi(next: boolean) {
  const user = detail.value?.user
  if (!user) return
  const updated = await run(() => unwrap(api.PUT('/v1/admin/users/{id}/ai-agent', {
    params: { path: { id: user.id } },
    body: { isAiAgent: next },
  })), next ? '已标记为 AI 账号' : '已取消 AI 账号标记')
  if (updated) {
    detail.value = updated
    load()
  }
}

async function passExam() {
  const user = detail.value?.user
  if (!user) return
  const ok = await confirm({
    title: `手动通过 @${user.username} 的入站考试？`,
    description: '不会发放首次通过奖励；会按现有经验重算等级。',
  })
  if (!ok) return
  const updated = await run(() => unwrap(api.POST('/v1/admin/users/{id}/exam-pass', {
    params: { path: { id: user.id } },
  })), '已标记为通过')
  if (updated) {
    detail.value = updated
    load()
  }
}

const dennyAmount = ref<number | ''>('')
const dennyReason = ref('')
const expAmount = ref<number | ''>('')
const expReason = ref('')
const blockNote = ref('')

function resetForms() {
  dennyAmount.value = ''
  dennyReason.value = ''
  expAmount.value = ''
  expReason.value = ''
  blockNote.value = ''
}

async function adjustDenny() {
  const user = detail.value?.user
  const amount = Number(dennyAmount.value)
  if (!user || !Number.isInteger(amount) || amount === 0) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/denny', {
    params: { path: { id: user.id } },
    body: { amount, reason: dennyReason.value },
  })), amount > 0 ? `已发放 ${amount} 丁尼` : `已扣除 ${-amount} 丁尼`)
  if (done && detail.value) {
    detail.value = { ...detail.value, denny: done.denny, exp: done.exp, user: { ...detail.value.user, level: done.level } }
    dennyAmount.value = ''
    dennyReason.value = ''
  }
}

async function grantExp() {
  const user = detail.value?.user
  const amount = Number(expAmount.value)
  if (!user || !Number.isInteger(amount) || amount <= 0) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/exp', {
    params: { path: { id: user.id } },
    body: { amount, reason: expReason.value },
  })), `已发放 ${amount} 经验`)
  if (done && detail.value) {
    detail.value = { ...detail.value, denny: done.denny, exp: done.exp, user: { ...detail.value.user, level: done.level } }
    expAmount.value = ''
    expReason.value = ''
    load()
  }
}

async function block() {
  const user = detail.value?.user
  if (!user) return
  const ok = await confirm({
    title: `封禁 @${user.username}？`,
    description: '对方的所有会话会立即失效，无法再登录；可以随时解封。',
    confirmLabel: '封禁',
    destructive: true,
    typeToConfirm: '确认',
  })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/block', {
    params: { path: { id: user.id } },
    body: { note: blockNote.value.trim() || null },
  })))
  if (done) {
    success(done.blocked ? '已封禁' : '该账号已处于封禁状态')
    blockNote.value = ''
    await refreshDetail()
  }
}

async function unblock() {
  const user = detail.value?.user
  if (!user) return
  const ok = await confirm({ title: `解封 @${user.username}？` })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/unblock', {
    params: { path: { id: user.id } },
  })))
  if (done) {
    success(done.unblocked ? '已解封' : '该账号未被封禁')
    await refreshDetail()
  }
}
</script>

<template>
  <div>
    <AdminPageHeader title="用户管理" :description="isAdmin ? '调整角色、丁尼和经验都会记入审计日志。' : '版主可以查看用户并封禁 / 解封；其他调整需要管理员。'">
      <Button variant="outline" size="sm" :disabled="loading" @click="load">
        <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />刷新
      </Button>
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-5">
      <Input v-model="q" placeholder="用户名 / 邮箱 / ID" class="lg:col-span-2" />
      <Select v-model="role">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部角色</SelectItem>
          <SelectItem v-for="(label, key) in roleLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="status">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem v-for="(label, key) in statusLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="ai">
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
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="user in result?.items ?? []" :key="user.id">
            <TableCell>
              <button class="font-medium hover:underline" @click="openDetail(user)">@{{ user.username }}</button>
              <Badge v-if="user.isAiAgent" variant="secondary" class="ml-2">AI</Badge>
              <div class="text-xs text-muted-foreground">ID {{ user.id }}</div>
            </TableCell>
            <TableCell class="hidden lg:table-cell">{{ user.email ?? '—' }}</TableCell>
            <TableCell>
              <Badge :variant="user.role === 'user' ? 'outline' : 'default'">{{ roleLabel[user.role] }}</Badge>
            </TableCell>
            <TableCell class="hidden md:table-cell text-right tabular-nums">Lv.{{ user.level }}</TableCell>
            <TableCell class="hidden md:table-cell">
              <span v-if="user.examPassedAt">已通过</span>
              <span v-else class="text-muted-foreground">未通过</span>
            </TableCell>
            <TableCell><Badge :variant="statusVariant(user.status)">{{ statusLabel[user.status] ?? user.status }}</Badge></TableCell>
            <TableCell class="hidden lg:table-cell">{{ formatTime(user.createdAt) }}</TableCell>
          </TableRow>
          <TableRow v-if="result && result.items.length === 0">
            <TableCell colspan="7" class="py-10 text-center text-muted-foreground">没有符合条件的用户</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="人" @change="(p) => { page = p; load() }" />

    <Sheet v-model:open="open">
      <SheetContent class="w-full overflow-y-auto sm:max-w-lg">
        <template v-if="detail">
          <SheetHeader>
            <div class="flex flex-wrap gap-2">
              <Badge :variant="detail.user.role === 'user' ? 'outline' : 'default'">{{ roleLabel[detail.user.role] }}</Badge>
              <Badge :variant="statusVariant(detail.user.status)">{{ statusLabel[detail.user.status] ?? detail.user.status }}</Badge>
              <Badge v-if="detail.user.isAiAgent" variant="secondary">AI 账号</Badge>
            </div>
            <SheetTitle>@{{ detail.user.username }}</SheetTitle>
            <SheetDescription>ID {{ detail.user.id }} · {{ detail.user.email ?? '未绑定邮箱' }}</SheetDescription>
          </SheetHeader>

          <div class="space-y-6 px-4 pb-6">
            <dl class="grid grid-cols-2 gap-3 text-sm">
              <div><dt class="text-muted-foreground">等级</dt><dd class="font-medium">Lv.{{ detail.user.level }}</dd></div>
              <div><dt class="text-muted-foreground">经验</dt><dd class="font-medium tabular-nums">{{ formatNumber(detail.exp) }}</dd></div>
              <div><dt class="text-muted-foreground">丁尼</dt><dd class="font-medium tabular-nums">{{ formatNumber(detail.denny) }}</dd></div>
              <div><dt class="text-muted-foreground">公开帖子</dt><dd class="font-medium tabular-nums">{{ formatNumber(detail.postCount) }}</dd></div>
              <div><dt class="text-muted-foreground">总浏览</dt><dd class="font-medium tabular-nums">{{ formatNumber(detail.totalViews) }}</dd></div>
              <div><dt class="text-muted-foreground">总获赞</dt><dd class="font-medium tabular-nums">{{ formatNumber(detail.totalLikes) }}</dd></div>
              <div><dt class="text-muted-foreground">注册时间</dt><dd>{{ formatTime(detail.user.createdAt) }}</dd></div>
              <div><dt class="text-muted-foreground">考试通过</dt><dd>{{ formatTime(detail.user.examPassedAt) }}</dd></div>
            </dl>
            <a :href="`/profile/${detail.user.id}`" target="_blank" class="text-sm underline">查看主页</a>

            <template v-if="isAdmin">
              <Separator />
              <section class="space-y-4">
                <h3 class="text-sm font-medium">账号设置</h3>
                <div class="flex items-center justify-between gap-4">
                  <Label>角色</Label>
                  <Select :model-value="detail.user.role" :disabled="isSelf || !editable" @update:model-value="(v) => changeRole(v as Role)">
                    <SelectTrigger class="w-40"><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="(label, key) in roleLabel" :key="key" :value="key">{{ label }}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <p v-if="isSelf" class="text-xs text-muted-foreground">不能修改自己的角色。</p>
                <div class="flex items-center justify-between gap-4">
                  <div>
                    <Label>AI 账号</Label>
                    <p class="text-xs text-muted-foreground">AI 角色绑定的账号；发出的消息不产生通知。</p>
                  </div>
                  <Switch :model-value="detail.user.isAiAgent" :disabled="!editable" @update:model-value="changeAi" />
                </div>
                <div class="flex items-center justify-between gap-4">
                  <Label>入站考试</Label>
                  <Button size="sm" variant="outline" :disabled="!!detail.user.examPassedAt || !editable" @click="passExam">
                    {{ detail.user.examPassedAt ? '已通过' : '手动通过' }}
                  </Button>
                </div>
              </section>

              <Separator />
              <section class="space-y-3">
                <h3 class="text-sm font-medium">调整丁尼</h3>
                <p class="text-xs text-muted-foreground">正数发放、负数扣除；余额封顶 9999，不能扣成负数。</p>
                <div class="flex gap-2">
                  <Input v-model.number="dennyAmount" type="number" placeholder="如 100 或 -50" class="w-48" />
                  <Input v-model="dennyReason" placeholder="原因（必填）" maxlength="200" />
                </div>
                <Button size="sm" :disabled="!editable || !dennyAmount || !dennyReason.trim()" @click="adjustDenny">提交</Button>
              </section>

              <section class="space-y-3">
                <h3 class="text-sm font-medium">发放经验</h3>
                <p class="text-xs text-muted-foreground">不占每日经验上限，发放后按新经验重算等级。</p>
                <div class="flex gap-2">
                  <Input v-model.number="expAmount" type="number" min="1" max="100000" placeholder="1–100000" class="w-48" />
                  <Input v-model="expReason" placeholder="原因（必填）" maxlength="200" />
                </div>
                <Button size="sm" :disabled="!editable || !expAmount || !expReason.trim()" @click="grantExp">提交</Button>
              </section>
            </template>

            <template v-if="!isSelf && detail.user.status !== 'deleted' && detail.user.status !== 'merged'">
              <Separator />
              <section class="space-y-3">
                <h3 class="text-sm font-medium">封禁</h3>
                <template v-if="detail.user.status === 'blocked'">
                  <Button size="sm" variant="outline" @click="unblock">解封</Button>
                </template>
                <template v-else>
                  <Textarea v-model="blockNote" placeholder="备注（选填，写入审核日志）" maxlength="500" rows="2" />
                  <Button size="sm" variant="destructive" @click="block">封禁账号</Button>
                </template>
              </section>
            </template>
          </div>
        </template>
        <template v-else>
          <SheetHeader class="sr-only">
            <SheetTitle>用户详情</SheetTitle>
            <SheetDescription>加载中</SheetDescription>
          </SheetHeader>
          <div class="p-6 text-sm text-muted-foreground">加载中…</div>
        </template>
      </SheetContent>
    </Sheet>
  </div>
</template>
