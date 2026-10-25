<script setup lang="ts">
import { Ban, Brain, Pencil, Pin, Plus, RotateCcw, Trash2, Unlink } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogDescription, DialogFooter, DialogHeader, DialogScrollContent, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime } from '~/lib/format'

definePageMeta({ admin: true })

type Settings = Schemas['QqSettings']
type Bot = Schemas['QqBotView']
type Group = Schemas['QqGroupView']
type Identity = Schemas['QqIdentityView']
type SharedMemory = Schemas['SharedMemoryView']

const NONE = 'none'
const { run, failure } = useNotify()
const confirm = useConfirm()
const tab = useQueryParam<string>('tab', 'settings')
const loading = ref(false)
const failed = ref(false)
const saving = ref(false)
const settings = ref<Settings | null>(null)
const form = ref<Settings | null>(null)
const blockedWords = ref('')
const guestModel = ref(NONE)
const bots = ref<Bot[]>([])
const characters = ref<Schemas['AdminCharacter'][]>([])
const models = ref<Schemas['AdminModel'][]>([])
const qqCharacters = computed(() => characters.value.filter(c => c.enabled && c.qqEnabled))

function fillForm(value: Settings) {
  settings.value = value
  form.value = { ...value, blockedWords: [...value.blockedWords] }
  blockedWords.value = value.blockedWords.join('\n')
  guestModel.value = value.guestModelKey ?? NONE
}

async function load() {
  loading.value = true
  try {
    const [s, b, c, m] = await Promise.all([
      unwrap(api.GET('/v1/admin/qq/settings')),
      unwrap(api.GET('/v1/admin/qq/bots')),
      unwrap(api.GET('/v1/admin/ai/characters')),
      unwrap(api.GET('/v1/admin/ai/models')),
    ])
    fillForm(s)
    bots.value = b.items
    characters.value = c
    models.value = m
    failed.value = false
  }
  catch (error) {
    failed.value = true
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

async function saveSettings() {
  if (!form.value) return
  const input = {
    ...form.value,
    guestModelKey: guestModel.value === NONE ? null : guestModel.value,
    blockedWords: [...new Set(blockedWords.value.split('\n').map(w => w.trim()).filter(Boolean))],
  }
  saving.value = true
  try {
    const saved = await run(() => unwrap(api.PUT('/v1/admin/qq/settings', { body: input })), 'QQ 设置已保存')
    if (saved) fillForm(saved)
  }
  finally {
    saving.value = false
  }
}

function characterName(slug?: string | null) {
  if (!slug) return '未指定'
  return characters.value.find(c => c.slug === slug)?.displayName ?? slug
}

function botName(appId: string) {
  return bots.value.find(b => b.appId === appId)?.displayName || appId
}

const botDraft = ref<{ appId: string, displayName: string, defaultCardSlug: string, enabled: boolean } | null>(null)
const botSaving = ref(false)

function editBot(bot: Bot) {
  botDraft.value = { ...bot, defaultCardSlug: bot.defaultCardSlug ?? NONE }
}

async function saveBot() {
  const draft = botDraft.value
  if (!draft) return
  botSaving.value = true
  try {
    const saved = await run(() => unwrap(api.PUT('/v1/admin/qq/bots/{appId}', {
      params: { path: { appId: draft.appId } },
      body: {
        displayName: draft.displayName.trim(),
        defaultCardSlug: draft.defaultCardSlug === NONE ? null : draft.defaultCardSlug,
        enabled: draft.enabled,
      },
    })), '机器人已保存')
    if (saved) {
      bots.value = bots.value.map(b => b.appId === saved.appId ? saved : b)
      botDraft.value = null
    }
  }
  finally {
    botSaving.value = false
  }
}

const groupList = useListQuery({
  filters: {},
  fetch: (_filters, page) => unwrap(api.GET('/v1/admin/qq/groups', { params: { query: { page, limit: 20 } } })),
})
const groups = computed(() => groupList.result.value?.items ?? [])
const groupDraft = ref<{ id: string, appId: string, displayName: string, cardSlug: string, enabled: boolean, blocked: boolean } | null>(null)
const groupSaving = ref(false)

function editGroup(group: Group) {
  groupDraft.value = { ...group, cardSlug: group.cardSlug ?? NONE }
}

async function saveGroup() {
  const draft = groupDraft.value
  if (!draft) return
  groupSaving.value = true
  try {
    const saved = await run(() => unwrap(api.PUT('/v1/admin/qq/groups/{id}', {
      params: { path: { id: draft.id } },
      body: {
        displayName: draft.displayName.trim(),
        cardSlug: draft.cardSlug === NONE ? null : draft.cardSlug,
        enabled: draft.enabled,
        blocked: draft.blocked,
        resetContext: false,
      },
    })), '群设置已保存')
    if (saved) {
      groupDraft.value = null
      await groupList.refresh()
    }
  }
  finally {
    groupSaving.value = false
  }
}

async function resetGroup(group: Group) {
  const ok = await confirm({
    title: `重置「${group.displayName}」的上下文？`,
    description: '之后的回复不再引用此前的群聊上下文，已保存的聊天记录仍保留。',
    confirmLabel: '重置上下文',
  })
  if (!ok) return
  const saved = await run(() => unwrap(api.PUT('/v1/admin/qq/groups/{id}', {
    params: { path: { id: group.id } },
    body: { displayName: group.displayName, cardSlug: group.cardSlug, enabled: group.enabled, blocked: group.blocked, resetContext: true },
  })), '群上下文已重置')
  if (saved) await groupList.refresh()
}

// ---------- 绑定身份（ADR 0008 第二期） ----------

const identityList = useListQuery({
  filters: { user: '' },
  text: ['user'],
  pageKey: 'identityPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/qq/identities', {
    params: { query: { user: optional(f.user), page, limit: 20 } },
  })),
})
const identities = computed(() => identityList.result.value?.items ?? [])

async function toggleIdentityBlock(identity: Identity) {
  const ok = await confirm({
    title: identity.blocked ? '解封这个 QQ 身份？' : '封禁这个 QQ 身份？',
    description: identity.blocked ? '解封后机器人会照常回复。' : '封禁后机器人不再回复这个身份（不影响站内账号）。',
    confirmLabel: identity.blocked ? '解封' : '封禁',
  })
  if (!ok) return
  const done = await run(() => unwrap(api.PUT('/v1/admin/qq/identities/{id}', {
    params: { path: { id: identity.id } },
    body: { blocked: !identity.blocked, unbind: false },
  })).then(() => true), identity.blocked ? '已解封' : '已封禁')
  if (done) await identityList.refresh()
}

async function unbindIdentity(identity: Identity) {
  const ok = await confirm({
    title: '强制解绑这个 QQ 身份？',
    description: '解绑后按游客的免费次数使用，机器人不再读写这个账号的记忆。已有的记忆不受影响。',
    confirmLabel: '解绑',
  })
  if (!ok) return
  const done = await run(() => unwrap(api.PUT('/v1/admin/qq/identities/{id}', {
    params: { path: { id: identity.id } },
    body: { blocked: identity.blocked, unbind: true },
  })).then(() => true), '已解绑')
  if (done) await identityList.refresh()
}

// ---------- 回复记录 ----------

const turnList = useListQuery({
  filters: { status: 'all' },
  pageKey: 'turnPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/qq/turns', {
    params: { query: { status: optional(f.status), page, limit: 20 } },
  })),
})
const turns = computed(() => turnList.result.value?.items ?? [])
const turnStatus: Record<string, string> = {
  queued: '排队中',
  processing: '处理中',
  completed: '已回复',
  failed: '失败',
  skipped: '跳过',
}

// ---------- 群共享记忆（ADR 0008 第三期） ----------

const memoryGroup = ref<Group | null>(null)
const memories = ref<SharedMemory[]>([])
const memoriesLoading = ref(false)
const memoryDraft = ref({ character: NONE, content: '' })
const memoryEdit = ref<{ id: string, content: string } | null>(null)

async function openMemories(group: Group) {
  memoryGroup.value = group
  memoryDraft.value = { character: group.cardSlug ?? qqCharacters.value[0]?.slug ?? NONE, content: '' }
  memoryEdit.value = null
  await loadMemories()
}

async function loadMemories() {
  const group = memoryGroup.value
  if (!group) return
  memoriesLoading.value = true
  try {
    memories.value = (await unwrap(api.GET('/v1/admin/qq/groups/{id}/memories', { params: { path: { id: group.id } } }))).items
  }
  catch (error) {
    failure(error)
  }
  finally {
    memoriesLoading.value = false
  }
}

async function addMemory() {
  const group = memoryGroup.value
  const draft = memoryDraft.value
  if (!group || !draft.content.trim() || draft.character === NONE) return
  const saved = await run(() => unwrap(api.POST('/v1/admin/qq/groups/{id}/memories', {
    params: { path: { id: group.id } },
    body: { character: draft.character, content: draft.content.trim(), pinned: false },
  })), '已记下')
  if (saved) {
    memoryDraft.value.content = ''
    await loadMemories()
  }
}

async function saveMemory(memory: SharedMemory, body: { content?: string, pinned?: boolean }) {
  const group = memoryGroup.value
  if (!group) return
  const saved = await run(() => unwrap(api.PUT('/v1/admin/qq/groups/{id}/memories/{memoryId}', {
    params: { path: { id: group.id, memoryId: memory.id } },
    body,
  })), '已保存')
  if (saved) {
    memoryEdit.value = null
    await loadMemories()
  }
}

async function deleteMemory(memory: SharedMemory) {
  const group = memoryGroup.value
  if (!group) return
  const ok = await confirm({ title: '删除这条群共享记忆？', description: memory.content, confirmLabel: '删除', destructive: true })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/qq/groups/{id}/memories/{memoryId}', {
    params: { path: { id: group.id, memoryId: memory.id } },
  })).then(() => true), '已删除')
  if (done) await loadMemories()
}

const memoryOrigin: Record<string, string> = { extracted: '自动提炼', told: '群友让记的', manual: '管理员' }

function refresh() {
  load()
  groupList.refresh()
  identityList.refresh()
  turnList.refresh()
}
</script>

<template>
  <div>
    <AdminPageHeader title="QQ 机器人" description="管理 QQ 群聊、单聊的角色与游客额度。每日额度在北京时间 04:00 重置。">
      <AdminRefreshButton :loading="loading || groupList.loading.value" @click="refresh" />
    </AdminPageHeader>

    <Tabs v-model="tab" class="space-y-4">
      <TabsList>
        <TabsTrigger value="settings">设置与机器人</TabsTrigger>
        <TabsTrigger value="groups">群管理</TabsTrigger>
        <TabsTrigger value="identities">绑定身份</TabsTrigger>
        <TabsTrigger value="turns">回复记录</TabsTrigger>
      </TabsList>

      <TabsContent value="settings" class="space-y-4">
        <Card>
          <CardHeader>
            <CardTitle>对话设置</CardTitle>
            <CardDescription>游客不读写记忆；成功发送回复才计一次，生成的 token 同时计入全站 AI 额度。</CardDescription>
          </CardHeader>
          <CardContent>
            <div v-if="failed && !form" class="flex items-center gap-3 text-sm text-muted-foreground">
              设置加载失败。
              <Button variant="outline" size="sm" @click="load">重试</Button>
            </div>
            <Skeleton v-else-if="!form" class="h-64 w-full" />
            <form v-else class="space-y-6" @submit.prevent="saveSettings">
              <label class="flex max-w-md items-center justify-between gap-4 text-sm font-medium">
                启用 QQ 对话
                <Switch v-model="form.enabled" />
              </label>
              <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
                <label class="grid gap-2">
                  <span class="text-sm font-medium">每个游客每日次数</span>
                  <Input v-model.number="form.guestDailyTurns" type="number" min="0" max="1000" required />
                  <span class="text-xs text-muted-foreground">0 表示禁止游客对话。</span>
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">每群每日游客次数</span>
                  <Input v-model.number="form.guestGroupDailyTurns" type="number" min="0" max="100000" required />
                  <span class="text-xs text-muted-foreground">限制同一个群中所有游客的总次数。</span>
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">所有游客每日加权 token</span>
                  <Input v-model.number="form.guestDailyTokens" type="number" min="0" required />
                  <span class="text-xs text-muted-foreground">0 表示不另设游客上限，全站上限仍生效。</span>
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">游客模型</span>
                  <Select v-model="guestModel">
                    <SelectTrigger><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem :value="NONE">跟随角色默认模型</SelectItem>
                      <SelectItem v-for="m in models.filter(m => m.enabled)" :key="m.key" :value="m.key">{{ m.displayName }}</SelectItem>
                    </SelectContent>
                  </Select>
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">生成超时（秒）</span>
                  <Input v-model.number="form.replyTimeoutSecs" type="number" min="10" max="180" required />
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">单次回复总字数上限</span>
                  <Input v-model.number="form.maxReplyChars" type="number" min="1" max="4000" required />
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">群聊上下文条数</span>
                  <Input v-model.number="form.groupContextTurns" type="number" min="1" max="30" required />
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">群聊上下文时长（小时）</span>
                  <Input v-model.number="form.groupContextHours" type="number" min="1" max="24" required />
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">单聊上下文条数</span>
                  <Input v-model.number="form.c2cContextTurns" type="number" min="1" max="60" required />
                </label>
              </div>
              <div class="grid gap-4 lg:grid-cols-2">
                <label class="grid content-start gap-2">
                  <span class="text-sm font-medium">回复里的链接</span>
                  <Select v-model="form.linkMode">
                    <SelectTrigger><SelectValue /></SelectTrigger>
                    <SelectContent>
                      <SelectItem value="strip">移除链接（默认）</SelectItem>
                      <SelectItem value="domain">仅保留绳网帖子链接</SelectItem>
                    </SelectContent>
                  </Select>
                  <span class="text-xs text-muted-foreground">保留链接前，需在 QQ 开放平台登记 interk.net 并确认能正常发送。</span>
                </label>
                <label class="grid gap-2">
                  <span class="text-sm font-medium">敏感词（每行一个）</span>
                  <Textarea v-model="blockedWords" rows="5" placeholder="回复命中任意词时，改为简短的拒绝回复" />
                  <span class="text-xs text-muted-foreground">最多 500 个，每个不超过 100 字。</span>
                </label>
              </div>
              <div class="flex flex-wrap items-center justify-between gap-3">
                <span class="text-xs text-muted-foreground">最近修改：{{ formatTime(settings?.updatedAt) }}</span>
                <Button type="submit" :disabled="saving || loading">保存设置</Button>
              </div>
            </form>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>机器人</CardTitle>
            <CardDescription>只显示部署配置中的机器人。默认角色需先在「AI 助手 → 角色」开启 QQ 对话。</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow><TableHead>名称 / AppID</TableHead><TableHead>默认角色</TableHead><TableHead>状态</TableHead><TableHead class="w-12" /></TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="bot in bots" :key="bot.appId">
                  <TableCell><div class="font-medium">{{ bot.displayName || '未命名机器人' }}</div><div class="font-mono text-xs text-muted-foreground">{{ bot.appId }}</div></TableCell>
                  <TableCell>{{ characterName(bot.defaultCardSlug) }}</TableCell>
                  <TableCell><Badge :variant="bot.enabled ? 'secondary' : 'outline'">{{ bot.enabled ? '启用' : '停用' }}</Badge></TableCell>
                  <TableCell><AdminRowActions :actions="[{ label: '编辑机器人', icon: Pencil, onSelect: () => editBot(bot) }]" /></TableCell>
                </TableRow>
                <AdminListState :colspan="4" :loading="loading" :empty="!bots.length" :failed="failed" empty-text="尚未配置机器人，请在服务器部署配置中添加 QQ AppID 与密钥。" @retry="load" />
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="groups">
        <Card>
          <CardHeader>
            <CardTitle>群管理</CardTitle>
            <CardDescription>群角色覆盖机器人默认角色。展示名仅用于后台，今日计数包含正在生成的预占。</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow><TableHead>群 / 机器人</TableHead><TableHead>角色</TableHead><TableHead>状态</TableHead><TableHead class="text-right">今日游客次数</TableHead><TableHead class="text-right">今日 token</TableHead><TableHead>最后活跃</TableHead><TableHead class="w-12" /></TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="group in groups" :key="group.id">
                  <TableCell><div class="font-medium">{{ group.displayName }}</div><div class="text-xs text-muted-foreground">{{ botName(group.appId) }}</div></TableCell>
                  <TableCell>{{ group.cardSlug ? characterName(group.cardSlug) : '跟随机器人' }}</TableCell>
                  <TableCell>
                    <div class="flex flex-wrap gap-1">
                      <Badge v-if="group.leftAt" variant="outline">已退群</Badge>
                      <Badge v-if="group.blocked" variant="destructive">已封禁</Badge>
                      <Badge v-else :variant="group.enabled ? 'secondary' : 'outline'">{{ group.enabled ? '启用' : '停用' }}</Badge>
                    </div>
                  </TableCell>
                  <TableCell class="text-right tabular-nums">{{ formatNumber(group.todayGuestTurns) }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{ formatNumber(group.todayTokens) }}</TableCell>
                  <TableCell class="text-sm text-muted-foreground">{{ formatTime(group.lastActiveAt) }}</TableCell>
                  <TableCell>
                    <AdminRowActions :actions="[
                      { label: '编辑群设置', icon: Pencil, onSelect: () => editGroup(group) },
                      { label: '群共享记忆', icon: Brain, onSelect: () => openMemories(group) },
                      { label: '重置上下文', icon: RotateCcw, onSelect: () => resetGroup(group) },
                    ]" />
                  </TableCell>
                </TableRow>
                <AdminListState :colspan="7" :loading="groupList.loading.value" :empty="!groups.length" :failed="groupList.failed.value" empty-text="尚未收到群事件，添加机器人到 QQ 群后会显示在这里。" @retry="groupList.refresh" />
              </TableBody>
            </Table>
            <AdminPager v-if="groupList.result.value" :page="groupList.page.value" :limit="groupList.result.value.limit" :total="groupList.result.value.total" unit="个群" @change="groupList.setPage" />
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="identities">
        <Card>
          <CardHeader>
            <CardTitle>绑定身份</CardTitle>
            <CardDescription>同一个人在每个群、单聊都是独立的 QQ 身份，各绑一次。openid 只显示后 6 位。</CardDescription>
          </CardHeader>
          <CardContent>
            <div class="mb-4 max-w-sm"><Input v-model="identityList.filters.user" placeholder="按绳网用户 ID 或用户名筛选" /></div>
            <Table>
              <TableHeader>
                <TableRow><TableHead>场景</TableHead><TableHead>群昵称 / openid</TableHead><TableHead>绑定账号</TableHead><TableHead>状态</TableHead><TableHead>最后活跃</TableHead><TableHead class="w-12" /></TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="identity in identities" :key="identity.id">
                  <TableCell>{{ identity.scene === 'group' ? (identity.groupName ?? '群') : '单聊' }}</TableCell>
                  <TableCell><div>{{ identity.nickname || '—' }}</div><div class="font-mono text-xs text-muted-foreground">…{{ identity.openidTail }}</div></TableCell>
                  <TableCell>
                    <span v-if="identity.userId" class="font-mono text-xs">{{ identity.userId }}</span>
                    <span v-else class="text-muted-foreground">游客</span>
                    <div v-if="identity.boundAt" class="text-xs text-muted-foreground">{{ formatTime(identity.boundAt) }} 绑定</div>
                  </TableCell>
                  <TableCell>
                    <div class="flex flex-wrap gap-1">
                      <Badge v-if="identity.blocked" variant="destructive">已封禁</Badge>
                      <Badge v-if="!identity.available" variant="outline">已删好友</Badge>
                    </div>
                  </TableCell>
                  <TableCell class="text-sm text-muted-foreground">{{ formatTime(identity.lastActiveAt) }}</TableCell>
                  <TableCell>
                    <AdminRowActions :actions="[
                      { label: identity.blocked ? '解封' : '封禁', icon: Ban, onSelect: () => toggleIdentityBlock(identity) },
                      ...(identity.userId ? [{ label: '强制解绑', icon: Unlink, onSelect: () => unbindIdentity(identity) }] : []),
                    ]" />
                  </TableCell>
                </TableRow>
                <AdminListState :colspan="6" :loading="identityList.loading.value" :empty="!identities.length" :failed="identityList.failed.value" empty-text="没有 QQ 身份。" @retry="identityList.refresh" />
              </TableBody>
            </Table>
            <AdminPager v-if="identityList.result.value" :page="identityList.page.value" :limit="identityList.result.value.limit" :total="identityList.result.value.total" unit="个身份" @change="identityList.setPage" />
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="turns">
        <Card>
          <CardHeader>
            <CardTitle>回复记录</CardTitle>
            <CardDescription>每条触发的消息一条记录，用于排查「机器人没反应」。不含消息正文。</CardDescription>
          </CardHeader>
          <CardContent>
            <div class="mb-4 max-w-xs">
              <Select v-model="turnList.filters.status">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">全部状态</SelectItem>
                  <SelectItem v-for="(label, key) in turnStatus" :key="key" :value="key">{{ label }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <Table>
              <TableHeader>
                <TableRow><TableHead>时间</TableHead><TableHead>场景</TableHead><TableHead>计费</TableHead><TableHead>状态 / 原因</TableHead><TableHead>模型</TableHead><TableHead class="text-right">token</TableHead></TableRow>
              </TableHeader>
              <TableBody>
                <TableRow v-for="turn in turns" :key="turn.id">
                  <TableCell class="text-sm text-muted-foreground">{{ formatTime(turn.createdAt) }}</TableCell>
                  <TableCell>{{ turn.scene === 'group' ? '群' : '单聊' }}</TableCell>
                  <TableCell>{{ turn.payerUserId ? '个人额度' : '游客' }}</TableCell>
                  <TableCell><Badge :variant="turn.status === 'failed' ? 'destructive' : 'secondary'">{{ turnStatus[turn.status] ?? turn.status }}</Badge><span v-if="turn.reason" class="ml-2 font-mono text-xs text-muted-foreground">{{ turn.reason }}</span></TableCell>
                  <TableCell class="font-mono text-xs">{{ turn.modelKey ?? '—' }}</TableCell>
                  <TableCell class="text-right tabular-nums">{{ formatNumber(turn.promptTokens + turn.completionTokens) }}</TableCell>
                </TableRow>
                <AdminListState :colspan="6" :loading="turnList.loading.value" :empty="!turns.length" :failed="turnList.failed.value" empty-text="还没有回复记录。" @retry="turnList.refresh" />
              </TableBody>
            </Table>
            <AdminPager v-if="turnList.result.value" :page="turnList.page.value" :limit="turnList.result.value.limit" :total="turnList.result.value.total" unit="条" @change="turnList.setPage" />
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>

    <Dialog :open="!!memoryGroup" @update:open="v => { if (!v) memoryGroup = null }">
      <DialogScrollContent v-if="memoryGroup" class="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>群共享记忆：{{ memoryGroup.displayName }}</DialogTitle>
          <DialogDescription>这个群大家共同的事（群规、约定、常聊的话题）。只在这个群里用，绑定用户提问时带上；置顶的不会被自动整理改掉。</DialogDescription>
        </DialogHeader>
        <div class="space-y-3">
          <Skeleton v-if="memoriesLoading && !memories.length" class="h-24 w-full" />
          <p v-else-if="!memories.length" class="text-sm text-muted-foreground">还没有共享记忆。</p>
          <div v-for="memory in memories" :key="memory.id" class="rounded-md border p-3">
            <div v-if="memoryEdit?.id === memory.id" class="space-y-2">
              <Textarea v-model="memoryEdit.content" rows="2" maxlength="200" />
              <div class="flex justify-end gap-2">
                <Button variant="outline" size="sm" @click="memoryEdit = null">取消</Button>
                <Button size="sm" @click="saveMemory(memory, { content: memoryEdit.content })">保存</Button>
              </div>
            </div>
            <div v-else class="flex items-start justify-between gap-3">
              <div>
                <div class="text-sm">{{ memory.content }}</div>
                <div class="mt-1 flex flex-wrap gap-1 text-xs text-muted-foreground">
                  <Badge v-if="memory.pinned" variant="secondary">置顶</Badge>
                  <span>{{ characterName(memory.character) }} · {{ memoryOrigin[memory.origin] ?? memory.origin }} · {{ formatTime(memory.updatedAt) }}</span>
                </div>
              </div>
              <AdminRowActions :actions="[
                { label: '编辑', icon: Pencil, onSelect: () => { memoryEdit = { id: memory.id, content: memory.content } } },
                { label: memory.pinned ? '取消置顶' : '置顶', icon: Pin, onSelect: () => saveMemory(memory, { pinned: !memory.pinned }) },
                { label: '删除', icon: Trash2, onSelect: () => deleteMemory(memory) },
              ]" />
            </div>
          </div>
          <form class="grid gap-2 border-t pt-3 sm:grid-cols-[10rem_1fr_auto]" @submit.prevent="addMemory">
            <Select v-model="memoryDraft.character">
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem v-for="c in qqCharacters" :key="c.slug" :value="c.slug">{{ c.displayName }}</SelectItem></SelectContent>
            </Select>
            <Input v-model="memoryDraft.content" maxlength="200" placeholder="例如：这个群周五晚上一起打本" />
            <Button type="submit" :disabled="!memoryDraft.content.trim() || memoryDraft.character === NONE"><Plus class="size-4" />记下</Button>
          </form>
        </div>
      </DialogScrollContent>
    </Dialog>

    <Dialog :open="!!botDraft" @update:open="v => { if (!v && !botSaving) botDraft = null }">
      <DialogScrollContent v-if="botDraft">
        <DialogHeader><DialogTitle>编辑机器人</DialogTitle><DialogDescription>AppID：{{ botDraft.appId }}</DialogDescription></DialogHeader>
        <form class="space-y-4" @submit.prevent="saveBot">
          <label class="grid gap-2"><span class="text-sm font-medium">展示名</span><Input v-model="botDraft.displayName" maxlength="64" /></label>
          <label class="grid gap-2">
            <span class="text-sm font-medium">默认角色</span>
            <Select v-model="botDraft.defaultCardSlug">
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem :value="NONE">未指定</SelectItem><SelectItem v-for="c in qqCharacters" :key="c.slug" :value="c.slug">{{ c.displayName }}</SelectItem></SelectContent>
            </Select>
            <span class="text-xs text-muted-foreground">没有默认角色时，只有单独指定了角色的群能生成回复。</span>
          </label>
          <label class="flex items-center justify-between text-sm">启用机器人<Switch v-model="botDraft.enabled" /></label>
          <DialogFooter><Button type="button" variant="outline" :disabled="botSaving" @click="botDraft = null">取消</Button><Button type="submit" :disabled="botSaving">保存</Button></DialogFooter>
        </form>
      </DialogScrollContent>
    </Dialog>

    <Dialog :open="!!groupDraft" @update:open="v => { if (!v && !groupSaving) groupDraft = null }">
      <DialogScrollContent v-if="groupDraft">
        <DialogHeader><DialogTitle>编辑群设置</DialogTitle><DialogDescription>{{ botName(groupDraft.appId) }}</DialogDescription></DialogHeader>
        <form class="space-y-4" @submit.prevent="saveGroup">
          <label class="grid gap-2"><span class="text-sm font-medium">群展示名</span><Input v-model="groupDraft.displayName" maxlength="64" placeholder="留空使用默认群名" /></label>
          <label class="grid gap-2">
            <span class="text-sm font-medium">群角色</span>
            <Select v-model="groupDraft.cardSlug">
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent><SelectItem :value="NONE">跟随机器人默认角色</SelectItem><SelectItem v-for="c in qqCharacters" :key="c.slug" :value="c.slug">{{ c.displayName }}</SelectItem></SelectContent>
            </Select>
          </label>
          <label class="flex items-center justify-between text-sm">启用群对话<Switch v-model="groupDraft.enabled" /></label>
          <label class="flex items-center justify-between text-sm">封禁这个群<Switch v-model="groupDraft.blocked" /></label>
          <DialogFooter><Button type="button" variant="outline" :disabled="groupSaving" @click="groupDraft = null">取消</Button><Button type="submit" :disabled="groupSaving">保存</Button></DialogFooter>
        </form>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
