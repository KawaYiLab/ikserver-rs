<script setup lang="ts">
import { ArrowLeft, Ban, Eraser, ExternalLink, LogOut, PenLine, ShieldCheck, UserRound } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { ApiError, api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime, roleLabel, roleVariant, statusLabel, userStatusVariant } from '~/lib/format'

type Role = Schemas['Role']

const route = useRoute()
const id = computed(() => String(route.params.id))
const { me, isAdmin } = useSession()
const siteLink = useSiteLink()
const { run, failure, success } = useNotify()
const confirm = useConfirm()
const tab = useQueryParam<string>('tab', 'overview')

const detail = ref<Schemas['AdminUserDetail'] | null>(null)
const missing = ref(false)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    detail.value = await unwrap(api.GET('/v1/admin/users/{id}', { params: { path: { id: id.value } } }))
    missing.value = false
  }
  catch (error) {
    if (error instanceof ApiError && error.status === 404) missing.value = true
    else failure(error)
  }
  finally {
    loading.value = false
  }
}
watch(id, () => {
  detail.value = null
  load()
}, { immediate: true })

const user = computed(() => detail.value?.user)
const isSelf = computed(() => user.value?.id === me.value?.id)
const isStaffTarget = computed(() => user.value?.role === 'moderator' || user.value?.role === 'admin')
/** 账号还在（没有注销、合并）。 */
const present = computed(() => user.value?.status === 'active' || user.value?.status === 'blocked')
const editable = computed(() => user.value?.status === 'active')
/** 版主不能处置版主、管理员（后端同样校验）。 */
const canSanction = computed(() => present.value && !isSelf.value && (isAdmin.value || !isStaffTarget.value))

useHead({ title: () => (user.value ? `@${user.value.username} · 账号 · 绳网管理后台` : '账号 · 绳网管理后台') })

// ---------- 账号设置（管理员） ----------
async function changeRole(next: Role) {
  const u = user.value
  if (!u || next === u.role) return
  const ok = await confirm({
    title: `把 @${u.username} 设为${roleLabel[next]}？`,
    description: next === 'user' ? '对方将失去后台权限。' : '对方将可以登录管理后台。',
  })
  if (!ok) return
  const updated = await run(() => unwrap(api.PUT('/v1/admin/users/{id}/role', {
    params: { path: { id: u.id } },
    body: { role: next },
  })), '角色已更新')
  if (updated) detail.value = updated
}

async function changeAi(next: boolean) {
  const u = user.value
  if (!u) return
  const updated = await run(() => unwrap(api.PUT('/v1/admin/users/{id}/ai-agent', {
    params: { path: { id: u.id } },
    body: { isAiAgent: next },
  })), next ? '已标记为 AI 账号' : '已取消 AI 账号标记')
  if (updated) detail.value = updated
}

async function passExam() {
  const u = user.value
  if (!u) return
  const ok = await confirm({
    title: `手动通过 @${u.username} 的入站考试？`,
    description: '不会发放首次通过奖励；会按现有经验重算等级。',
  })
  if (!ok) return
  const updated = await run(() => unwrap(api.POST('/v1/admin/users/{id}/exam-pass', {
    params: { path: { id: u.id } },
  })), '已标记为通过')
  if (updated) detail.value = updated
}

const dennyAmount = ref<number | ''>('')
const dennyReason = ref('')
const expAmount = ref<number | ''>('')
const expReason = ref('')
/** 流水标签页在改动后刷新。 */
const ledgerVersion = ref(0)

async function adjustDenny() {
  const u = user.value
  const amount = Number(dennyAmount.value)
  if (!u || !Number.isInteger(amount) || amount === 0) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/denny', {
    params: { path: { id: u.id } },
    body: { amount, reason: dennyReason.value },
  })), amount > 0 ? `已发放 ${amount} 丁尼` : `已扣除 ${-amount} 丁尼`)
  if (done) {
    dennyAmount.value = ''
    dennyReason.value = ''
    ledgerVersion.value++
    load()
  }
}

async function grantExp() {
  const u = user.value
  const amount = Number(expAmount.value)
  if (!u || !Number.isInteger(amount) || amount <= 0) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/exp', {
    params: { path: { id: u.id } },
    body: { amount, reason: expReason.value },
  })), `已发放 ${amount} 经验`)
  if (done) {
    expAmount.value = ''
    expReason.value = ''
    ledgerVersion.value++
    load()
  }
}

// ---------- 处置 ----------
const blockOpen = ref(false)
const blockNote = ref('')

async function block() {
  const u = user.value
  if (!u) return
  const ok = await confirm({
    title: `封禁 @${u.username}？`,
    description: '对方的所有会话会立即失效，无法再登录；可以随时解封。',
    confirmLabel: '封禁',
    destructive: true,
    typeToConfirm: '确认',
  })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/block', {
    params: { path: { id: u.id } },
    body: { note: blockNote.value.trim() || null },
  })))
  if (done) {
    success(done.blocked ? '已封禁' : '该账号已处于封禁状态')
    blockOpen.value = false
    blockNote.value = ''
    load()
  }
}

async function unblock() {
  const u = user.value
  if (!u) return
  const ok = await confirm({ title: `解封 @${u.username}？` })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/unblock', { params: { path: { id: u.id } } })))
  if (done) {
    success(done.unblocked ? '已解封' : '该账号未被封禁')
    load()
  }
}

async function forceLogout() {
  const u = user.value
  if (!u) return
  const ok = await confirm({
    title: `让 @${u.username} 在所有设备上下线？`,
    description: '吊销全部登录会话，在线的连接立即断开；对方可以重新登录。',
    confirmLabel: '强制下线',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/sessions/revoke', { params: { path: { id: u.id } } })))
  if (done) {
    success(done.revoked ? `已下线 ${done.revoked} 个会话` : '对方当前没有登录中的会话')
    ledgerVersion.value++
  }
}

const renameOpen = ref(false)
const renameForm = reactive({ username: '', note: '', notify: true })

function startRename() {
  renameForm.username = user.value?.username ?? ''
  renameForm.note = ''
  renameForm.notify = true
  renameOpen.value = true
}

async function rename() {
  const u = user.value
  if (!u) return
  const updated = await run(() => unwrap(api.PUT('/v1/admin/users/{id}/username', {
    params: { path: { id: u.id } },
    body: { username: renameForm.username, note: renameForm.note.trim() || null, notify: renameForm.notify },
  })), '已改名')
  if (updated) {
    detail.value = updated
    renameOpen.value = false
  }
}

const resetOpen = ref(false)
const resetForm = reactive({ bio: true, avatar: false, note: '', notify: true })

function startReset() {
  resetForm.bio = !!detail.value?.bio
  resetForm.avatar = !!detail.value?.avatarUrl
  resetForm.note = ''
  resetForm.notify = true
  resetOpen.value = true
}

async function resetProfile() {
  const u = user.value
  if (!u) return
  const done = await run(() => unwrap(api.POST('/v1/admin/users/{id}/profile/reset', {
    params: { path: { id: u.id } },
    body: { bio: resetForm.bio, avatar: resetForm.avatar, note: resetForm.note.trim() || null, notify: resetForm.notify },
  })))
  if (done) {
    success(done.bioCleared || done.avatarCleared ? '已清理' : '没有需要清理的资料')
    resetOpen.value = false
    load()
  }
}

const stats = computed(() => {
  const d = detail.value
  if (!d) return []
  return [
    { label: '等级', value: `Lv.${d.user.level}` },
    { label: '经验', value: formatNumber(d.exp) },
    { label: '丁尼', value: formatNumber(d.denny), hint: `累计投出 ${formatNumber(d.dennyGiven)}` },
    { label: '签到', value: `${formatNumber(d.checkInTotal)} 天`, hint: `连签 ${formatNumber(d.checkInStreak)} 天` },
    { label: '公开委托', value: formatNumber(d.postCount), hint: '实名、未隐藏' },
    { label: '总浏览', value: formatNumber(d.totalViews) },
    { label: '总获赞', value: formatNumber(d.totalLikes) },
    { label: '收到评论', value: formatNumber(d.totalComments) },
  ]
})
</script>

<template>
  <div>
    <NuxtLink to="/users" class="mb-4 inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground">
      <ArrowLeft class="size-4" />账号列表
    </NuxtLink>

    <div v-if="missing" class="rounded-md border p-10 text-center text-muted-foreground">
      账号 {{ id }} 不存在
    </div>

    <template v-else-if="detail && user">
      <!-- 页头 -->
      <div class="mb-6 flex flex-col gap-4 sm:flex-row sm:items-start">
        <div class="flex size-16 shrink-0 items-center justify-center overflow-hidden rounded-full border bg-muted">
          <img v-if="detail.avatarUrl" :src="detail.avatarUrl" alt="" class="size-full object-cover">
          <UserRound v-else class="size-8 text-muted-foreground" />
        </div>
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-2">
            <h1 class="truncate text-2xl font-semibold tracking-tight">@{{ user.username }}</h1>
            <Badge :variant="roleVariant(user.role)">{{ roleLabel[user.role] }}</Badge>
            <Badge :variant="userStatusVariant(user.status)">{{ statusLabel[user.status] ?? user.status }}</Badge>
            <Badge v-if="user.isAiAgent" variant="secondary">AI 账号</Badge>
            <Badge v-if="isSelf" variant="outline">我</Badge>
          </div>
          <p class="mt-1 text-sm text-muted-foreground">
            No.{{ user.memberNo }} · ID <span class="font-mono">{{ user.id }}</span> · {{ user.email ?? '未绑定邮箱' }} ·
            注册于 {{ formatTime(user.createdAt) }} · 最近活跃 {{ formatTime(detail.lastSeenAt) }}
          </p>
          <p v-if="detail.bio" class="mt-2 line-clamp-2 text-sm">「{{ detail.bio }}」</p>
        </div>
        <div class="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" as-child>
            <a :href="siteLink(`/profile/${user.id}`)" target="_blank" rel="noopener"><ExternalLink class="size-4" />主站主页</a>
          </Button>
          <Button v-if="canSanction" variant="outline" size="sm" @click="startReset">
            <Eraser class="size-4" />清理资料
          </Button>
          <Button v-if="isAdmin && canSanction" variant="outline" size="sm" @click="startRename">
            <PenLine class="size-4" />改名
          </Button>
          <Button v-if="isAdmin && canSanction" variant="outline" size="sm" @click="forceLogout">
            <LogOut class="size-4" />强制下线
          </Button>
          <template v-if="canSanction">
            <Button v-if="user.status === 'blocked'" variant="outline" size="sm" @click="unblock">
              <ShieldCheck class="size-4" />解封
            </Button>
            <Button v-else-if="!isStaffTarget" variant="destructive" size="sm" @click="blockOpen = true">
              <Ban class="size-4" />封禁
            </Button>
          </template>
        </div>
      </div>

      <Tabs v-model="tab">
        <div class="-mx-4 mb-4 overflow-x-auto px-4 md:mx-0 md:px-0">
          <TabsList>
            <TabsTrigger value="overview">概览</TabsTrigger>
            <TabsTrigger value="posts">委托</TabsTrigger>
            <TabsTrigger value="comments">评论</TabsTrigger>
            <TabsTrigger value="moderation">举报与处罚</TabsTrigger>
            <TabsTrigger value="denny">丁尼流水</TabsTrigger>
            <TabsTrigger value="exp">经验记录</TabsTrigger>
            <TabsTrigger v-if="isAdmin" value="sessions">登录设备</TabsTrigger>
            <TabsTrigger v-if="isAdmin" value="audit">操作记录</TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="overview" class="space-y-6">
          <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <AdminStatCard v-for="s in stats" :key="s.label" :title="s.label" :value="s.value" :hint="s.hint" />
          </div>

          <div v-if="isAdmin" class="grid gap-6 lg:grid-cols-2">
            <section class="space-y-4 rounded-lg border p-4">
              <h2 class="font-medium">账号设置</h2>
              <div class="flex items-center justify-between gap-4">
                <Label>角色</Label>
                <Select :model-value="user.role" :disabled="isSelf || !editable" @update:model-value="(v) => changeRole(v as Role)">
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
                <Switch :model-value="user.isAiAgent" :disabled="!editable" @update:model-value="changeAi" />
              </div>
              <div class="flex items-center justify-between gap-4">
                <div>
                  <Label>入站考试</Label>
                  <p class="text-xs text-muted-foreground">{{ user.examPassedAt ? `通过于 ${formatTime(user.examPassedAt)}` : '尚未通过' }}</p>
                </div>
                <Button size="sm" variant="outline" :disabled="!!user.examPassedAt || !editable" @click="passExam">
                  {{ user.examPassedAt ? '已通过' : '手动通过' }}
                </Button>
              </div>
            </section>

            <section class="space-y-4 rounded-lg border p-4">
              <h2 class="font-medium">丁尼与经验</h2>
              <div class="space-y-2">
                <Label>调整丁尼</Label>
                <p class="text-xs text-muted-foreground">正数发放、负数扣除；余额封顶 9999，不能扣成负数。</p>
                <div class="flex gap-2">
                  <Input v-model.number="dennyAmount" type="number" placeholder="±数量" class="w-28" />
                  <Input v-model="dennyReason" placeholder="原因（必填）" maxlength="200" />
                  <Button size="sm" class="h-9" :disabled="!editable || !dennyAmount || !dennyReason.trim()" @click="adjustDenny">提交</Button>
                </div>
              </div>
              <Separator />
              <div class="space-y-2">
                <Label>发放经验</Label>
                <p class="text-xs text-muted-foreground">不占每日经验上限，发放后按新经验重算等级。</p>
                <div class="flex gap-2">
                  <Input v-model.number="expAmount" type="number" min="1" max="100000" placeholder="1–100000" class="w-28" />
                  <Input v-model="expReason" placeholder="原因（必填）" maxlength="200" />
                  <Button size="sm" class="h-9" :disabled="!editable || !expAmount || !expReason.trim()" @click="grantExp">提交</Button>
                </div>
              </div>
            </section>
          </div>
        </TabsContent>

        <TabsContent value="posts">
          <AdminUserPosts :user-id="user.id" />
        </TabsContent>
        <TabsContent value="comments">
          <AdminUserComments :user-id="user.id" />
        </TabsContent>
        <TabsContent value="moderation">
          <AdminUserModeration :user-id="user.id" />
        </TabsContent>
        <TabsContent value="denny">
          <AdminUserLedger :key="ledgerVersion" :user-id="user.id" kind="denny" />
        </TabsContent>
        <TabsContent value="exp">
          <AdminUserLedger :key="ledgerVersion" :user-id="user.id" kind="exp" />
        </TabsContent>
        <TabsContent v-if="isAdmin" value="sessions">
          <AdminUserSessions :key="ledgerVersion" :user-id="user.id" />
        </TabsContent>
        <TabsContent v-if="isAdmin" value="audit">
          <AdminUserAudit :user-id="user.id" />
        </TabsContent>
      </Tabs>
    </template>

    <div v-else class="space-y-4">
      <div class="flex gap-4">
        <Skeleton class="size-16 rounded-full" />
        <div class="flex-1 space-y-2">
          <Skeleton class="h-7 w-48" />
          <Skeleton class="h-4 w-96 max-w-full" />
        </div>
      </div>
      <Skeleton class="h-9 w-full max-w-xl" />
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <Skeleton v-for="i in 8" :key="i" class="h-28" />
      </div>
    </div>

    <!-- 封禁 -->
    <Dialog v-model:open="blockOpen">
      <DialogContent v-if="user">
        <DialogHeader>
          <DialogTitle>封禁 @{{ user.username }}</DialogTitle>
          <DialogDescription>对方的所有会话会立即失效，无法再登录；可以随时解封。</DialogDescription>
        </DialogHeader>
        <Textarea v-model="blockNote" placeholder="备注（选填，写入审核日志）" maxlength="500" rows="3" />
        <DialogFooter>
          <Button variant="outline" @click="blockOpen = false">取消</Button>
          <Button variant="destructive" @click="block">下一步</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 改名 -->
    <Dialog v-model:open="renameOpen">
      <DialogContent v-if="user">
        <DialogHeader>
          <DialogTitle>修改 @{{ user.username }} 的用户名</DialogTitle>
          <DialogDescription>用于处理违规用户名：不收费，封禁中的账号也能改；规则与用户自己改名一致。</DialogDescription>
        </DialogHeader>
        <div class="space-y-4">
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">新用户名</span>
            <Input v-model="renameForm.username" maxlength="20" />
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">原因（选填，写入审计）</span>
            <Textarea v-model="renameForm.note" maxlength="200" rows="2" />
          </label>
          <label class="flex items-center gap-2 text-sm">
            <Checkbox v-model="renameForm.notify" />给对方发系统通知（附上原因）
          </label>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="renameOpen = false">取消</Button>
          <Button :disabled="!renameForm.username.trim() || renameForm.username.trim() === user.username" @click="rename">保存</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 清理资料 -->
    <Dialog v-model:open="resetOpen">
      <DialogContent v-if="user">
        <DialogHeader>
          <DialogTitle>清理 @{{ user.username }} 的资料</DialogTitle>
          <DialogDescription>清除违规的签名或自定义头像（不退还设置头像的丁尼）。系统头像与名片不受影响。</DialogDescription>
        </DialogHeader>
        <div class="space-y-4">
          <label class="flex items-start gap-2 text-sm">
            <Checkbox v-model="resetForm.bio" class="mt-0.5" />
            <span>清空签名<span v-if="detail?.bio" class="block text-xs text-muted-foreground">当前：{{ detail.bio }}</span><span v-else class="block text-xs text-muted-foreground">当前没有签名</span></span>
          </label>
          <label class="flex items-start gap-2 text-sm">
            <Checkbox v-model="resetForm.avatar" class="mt-0.5" />
            <span>清除自定义头像<span v-if="!detail?.avatarUrl" class="block text-xs text-muted-foreground">当前没有自定义头像</span></span>
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">原因（选填，写入审计）</span>
            <Textarea v-model="resetForm.note" maxlength="200" rows="2" />
          </label>
          <label class="flex items-center gap-2 text-sm">
            <Checkbox v-model="resetForm.notify" />给对方发系统通知（附上原因）
          </label>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="resetOpen = false">取消</Button>
          <Button variant="destructive" :disabled="!resetForm.bio && !resetForm.avatar" @click="resetProfile">清理</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
