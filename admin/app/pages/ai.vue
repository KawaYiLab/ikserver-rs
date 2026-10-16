<script setup lang="ts">
import { Plus, RefreshCw, X } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogFooter, DialogHeader, DialogScrollContent, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime } from '~/lib/format'

definePageMeta({ admin: true })

type Model = Schemas['AdminModel']
type Character = Schemas['AdminCharacter']

const { run, failure } = useNotify()
const confirm = useConfirm()
const tab = ref('settings')
const loading = ref(false)

const settings = ref<Schemas['AdminSettings'] | null>(null)
const models = ref<Model[] | null>(null)
const characters = ref<Character[] | null>(null)

async function loadConfig() {
  const [s, m, c] = await Promise.all([
    unwrap(api.GET('/v1/admin/ai/settings')),
    unwrap(api.GET('/v1/admin/ai/models')),
    unwrap(api.GET('/v1/admin/ai/characters')),
  ])
  settings.value = s
  settingsForm.value = { globalEnabled: s.globalEnabled, dailyTokens: s.dailyTokens, dailyTokensPerUser: s.dailyTokensPerUser }
  models.value = m
  characters.value = c
}

async function load() {
  loading.value = true
  try {
    await Promise.all([loadConfig(), loadJobs()])
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

// ---------- 全局设置 ----------
const settingsForm = ref<Schemas['SettingsInput']>({ globalEnabled: false, dailyTokens: 0, dailyTokensPerUser: 0 })

async function saveSettings() {
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/settings', { body: settingsForm.value })), '设置已保存')
  if (saved) settings.value = saved
}

// ---------- 模型 ----------
interface ModelDraft extends Required<Omit<Schemas['ModelInput'], 'badge'>> {
  badge: string
  isNew: boolean
}

const modelDraft = ref<ModelDraft | null>(null)

function editModel(model?: Model) {
  modelDraft.value = model
    ? { ...model, badge: model.badge ?? '', isNew: false }
    : {
        key: '',
        displayName: '',
        description: '',
        badge: '',
        provider: settings.value?.providers[0] ?? '',
        providerModel: '',
        costWeight: 1,
        enabled: true,
        isDefault: false,
        sortOrder: (models.value?.length ?? 0) * 10,
        isNew: true,
      }
}

async function saveModel() {
  const draft = modelDraft.value
  if (!draft) return
  const { isNew: _isNew, badge, ...rest } = draft
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/models/{key}', {
    params: { path: { key: draft.key } },
    body: { ...rest, badge: badge.trim() || null },
  })), '模型已保存')
  if (saved) {
    modelDraft.value = null
    await loadConfig().catch(failure)
  }
}

async function removeModel(model: Model) {
  const ok = await confirm({
    title: `删除模型「${model.displayName}」？`,
    description: '会从所有角色的可用模型里移除；默认模型不能删除。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/ai/models/{key}', { params: { path: { key: model.key } } })).then(() => true), '已删除')
  if (done) await loadConfig().catch(failure)
}

// ---------- 角色 ----------
interface CharacterDraft {
  isNew: boolean
  slug: string
  displayName: string
  bio: string
  username: string
  avatarId: string | null
  avatar: Schemas['MediaView'] | null
  enabled: boolean
  knockVisible: boolean
  dmEnabled: boolean
  commentEnabled: boolean
  sortOrder: number
  outputMode: Schemas['OutputMode']
  systemPrompt: string
  examples: Schemas['Example'][]
  defaultModelKey: string
  allowedModelKeys: string[]
  suggestedQuestions: string
}

const characterDraft = ref<CharacterDraft | null>(null)

watch(() => characterDraft.value?.avatar, (media) => {
  if (media && characterDraft.value) characterDraft.value.avatarId = media.id
})

function editCharacter(character?: Character) {
  characterDraft.value = character
    ? {
        isNew: false,
        slug: character.slug,
        displayName: character.displayName,
        bio: character.bio,
        username: character.username ?? '',
        avatarId: character.avatarId ?? null,
        avatar: null,
        enabled: character.enabled,
        knockVisible: character.knockVisible,
        dmEnabled: character.dmEnabled,
        commentEnabled: character.commentEnabled,
        sortOrder: character.sortOrder,
        outputMode: character.outputMode,
        systemPrompt: character.systemPrompt,
        examples: character.examples.map(e => ({ ...e })),
        defaultModelKey: character.defaultModelKey ?? 'none',
        allowedModelKeys: [...character.allowedModelKeys],
        suggestedQuestions: character.suggestedQuestions.join('\n'),
      }
    : {
        isNew: true,
        slug: '',
        displayName: '',
        bio: '',
        username: '',
        avatarId: null,
        avatar: null,
        enabled: false,
        knockVisible: true,
        dmEnabled: true,
        commentEnabled: true,
        sortOrder: (characters.value?.length ?? 0) * 10,
        outputMode: 'markdown',
        systemPrompt: '',
        examples: [],
        defaultModelKey: 'none',
        allowedModelKeys: [],
        suggestedQuestions: '',
      }
}

function toggleAllowed(key: string, on: boolean | 'indeterminate') {
  const draft = characterDraft.value
  if (!draft) return
  draft.allowedModelKeys = on === true
    ? [...new Set([...draft.allowedModelKeys, key])]
    : draft.allowedModelKeys.filter(k => k !== key)
}

async function saveCharacter() {
  const d = characterDraft.value
  if (!d) return
  const body: Schemas['CharacterInput'] = {
    slug: d.slug,
    displayName: d.displayName,
    bio: d.bio,
    username: d.username.trim(),
    avatarId: d.avatarId,
    enabled: d.enabled,
    knockVisible: d.knockVisible,
    dmEnabled: d.dmEnabled,
    commentEnabled: d.commentEnabled,
    sortOrder: d.sortOrder,
    outputMode: d.outputMode,
    systemPrompt: d.systemPrompt,
    examples: d.examples.filter(e => e.user.trim() || e.assistant.trim()),
    defaultModelKey: d.defaultModelKey === 'none' ? null : d.defaultModelKey,
    allowedModelKeys: d.allowedModelKeys,
    suggestedQuestions: d.suggestedQuestions.split('\n').map(q => q.trim()).filter(Boolean),
  }
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/characters/{slug}', {
    params: { path: { slug: d.slug } },
    body,
  })), '角色已保存')
  if (saved) {
    characterDraft.value = null
    await loadConfig().catch(failure)
  }
}

async function removeCharacter(character: Character) {
  const ok = await confirm({
    title: `删除角色「${character.displayName}」？`,
    description: '绑定的账号保留，但不再作为 AI 回复；已有的会话记录不受影响。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/ai/characters/{slug}', { params: { path: { slug: character.slug } } })).then(() => true), '已删除')
  if (done) await loadConfig().catch(failure)
}

// ---------- 任务 ----------
const jobStatus = ref('all')
const jobKind = ref('all')
const jobPage = ref(1)
const jobs = ref<Schemas['Paged_AdminJob'] | null>(null)
const jobStatusLabel: Record<string, string> = { queued: '排队', processing: '进行中', completed: '完成', failed: '失败', skipped: '跳过' }
const jobKindLabel: Record<string, string> = { dm: '私聊', comment: '评论' }

async function loadJobs() {
  jobs.value = await unwrap(api.GET('/v1/admin/ai/jobs', {
    params: {
      query: {
        status: jobStatus.value === 'all' ? undefined : jobStatus.value,
        kind: jobKind.value === 'all' ? undefined : jobKind.value,
        page: jobPage.value,
        limit: 50,
      },
    },
  }))
}

function reloadJobs() {
  loadJobs().catch(failure)
}

watch([jobStatus, jobKind], () => {
  jobPage.value = 1
  reloadJobs()
})

function jobVariant(status: string) {
  return status === 'failed' ? 'destructive' : status === 'completed' ? 'outline' : 'secondary'
}
</script>

<template>
  <div>
    <AdminPageHeader title="AI 助手" description="修改在本实例立即生效，其他实例最长 60 秒后生效。">
      <Button variant="outline" size="sm" :disabled="loading" @click="load">
        <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />刷新
      </Button>
      <Button v-if="tab === 'models'" size="sm" @click="editModel()">
        <Plus class="size-4" />新建模型
      </Button>
      <Button v-if="tab === 'characters'" size="sm" @click="editCharacter()">
        <Plus class="size-4" />新建角色
      </Button>
    </AdminPageHeader>

    <Tabs v-model="tab">
      <TabsList class="mb-4">
        <TabsTrigger value="settings">全局设置</TabsTrigger>
        <TabsTrigger value="models">模型</TabsTrigger>
        <TabsTrigger value="characters">角色</TabsTrigger>
        <TabsTrigger value="jobs">任务</TabsTrigger>
      </TabsList>

      <TabsContent value="settings">
        <Card class="max-w-xl">
          <CardHeader>
            <CardTitle>开关与额度</CardTitle>
            <CardDescription>额度按模型的成本权重加权计算 token；0 表示不限制。</CardDescription>
          </CardHeader>
          <CardContent class="space-y-4">
            <label class="flex items-center justify-between gap-4 text-sm">
              全站启用 AI
              <Switch v-model="settingsForm.globalEnabled" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">每人每日加权 token</span>
              <Input v-model.number="settingsForm.dailyTokensPerUser" type="number" min="0" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">全站每日加权 token</span>
              <Input v-model.number="settingsForm.dailyTokens" type="number" min="0" />
            </label>
            <div class="text-sm text-muted-foreground">
              已配置的提供方：
              <template v-if="settings?.providers.length">
                <Badge v-for="p in settings.providers" :key="p" variant="outline" class="mr-1">{{ p }}</Badge>
              </template>
              <span v-else>无（需要在部署配置里添加）</span>
            </div>
            <Button @click="saveSettings">保存</Button>
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="models">
        <div class="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="w-16 text-right">排序</TableHead>
                <TableHead>模型</TableHead>
                <TableHead class="hidden md:table-cell">上游</TableHead>
                <TableHead class="text-right">权重</TableHead>
                <TableHead>状态</TableHead>
                <TableHead class="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="model in models ?? []" :key="model.key">
                <TableCell class="text-right tabular-nums">{{ model.sortOrder }}</TableCell>
                <TableCell>
                  <div class="font-medium">
                    {{ model.displayName }}
                    <Badge v-if="model.badge" variant="secondary" class="ml-1">{{ model.badge }}</Badge>
                  </div>
                  <div class="font-mono text-xs text-muted-foreground">{{ model.key }}</div>
                </TableCell>
                <TableCell class="hidden md:table-cell">
                  <span class="font-mono text-xs">{{ model.provider }} / {{ model.providerModel }}</span>
                  <Badge v-if="!model.providerConfigured" variant="destructive" class="ml-1">提供方未配置</Badge>
                </TableCell>
                <TableCell class="text-right tabular-nums">×{{ model.costWeight }}</TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge :variant="model.enabled ? 'default' : 'outline'">{{ model.enabled ? '启用' : '停用' }}</Badge>
                    <Badge v-if="model.isDefault" variant="secondary">默认</Badge>
                  </div>
                </TableCell>
                <TableCell class="space-x-1 text-right whitespace-nowrap">
                  <Button size="sm" variant="outline" @click="editModel(model)">编辑</Button>
                  <Button size="sm" variant="ghost" class="text-destructive" :disabled="model.isDefault" @click="removeModel(model)">删除</Button>
                </TableCell>
              </TableRow>
              <TableRow v-if="models && models.length === 0">
                <TableCell colspan="6" class="py-10 text-center text-muted-foreground">还没有模型</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </TabsContent>

      <TabsContent value="characters">
        <div class="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="w-16 text-right">排序</TableHead>
                <TableHead>角色</TableHead>
                <TableHead class="hidden md:table-cell">绑定账号</TableHead>
                <TableHead>入口</TableHead>
                <TableHead>状态</TableHead>
                <TableHead class="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="character in characters ?? []" :key="character.slug">
                <TableCell class="text-right tabular-nums">{{ character.sortOrder }}</TableCell>
                <TableCell>
                  <div class="font-medium">{{ character.displayName }}</div>
                  <div class="font-mono text-xs text-muted-foreground">{{ character.slug }}</div>
                </TableCell>
                <TableCell class="hidden md:table-cell">
                  <span v-if="character.username">@{{ character.username }}</span>
                  <Badge v-else variant="destructive">账号不存在</Badge>
                </TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge v-if="character.dmEnabled" variant="outline">私聊</Badge>
                    <Badge v-if="character.commentEnabled" variant="outline">评论</Badge>
                    <Badge v-if="character.knockVisible" variant="outline">敲敲列表</Badge>
                  </div>
                </TableCell>
                <TableCell>
                  <Badge :variant="character.enabled ? 'default' : 'outline'">{{ character.enabled ? '启用' : '停用' }}</Badge>
                </TableCell>
                <TableCell class="space-x-1 text-right whitespace-nowrap">
                  <Button size="sm" variant="outline" @click="editCharacter(character)">编辑</Button>
                  <Button size="sm" variant="ghost" class="text-destructive" @click="removeCharacter(character)">删除</Button>
                </TableCell>
              </TableRow>
              <TableRow v-if="characters && characters.length === 0">
                <TableCell colspan="6" class="py-10 text-center text-muted-foreground">还没有角色</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </TabsContent>

      <TabsContent value="jobs">
        <div class="mb-4 grid gap-2 sm:grid-cols-4">
          <Select v-model="jobStatus">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部状态</SelectItem>
              <SelectItem v-for="(label, key) in jobStatusLabel" :key="key" :value="key">{{ label }}</SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="jobKind">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部类型</SelectItem>
              <SelectItem v-for="(label, key) in jobKindLabel" :key="key" :value="key">{{ label }}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div class="rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>时间</TableHead>
                <TableHead>类型</TableHead>
                <TableHead>状态</TableHead>
                <TableHead class="hidden md:table-cell">模型</TableHead>
                <TableHead class="hidden md:table-cell text-right">token（入 / 出）</TableHead>
                <TableHead class="hidden lg:table-cell">用户 / AI 账号</TableHead>
                <TableHead>原因</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="job in jobs?.items ?? []" :key="job.id">
                <TableCell class="whitespace-nowrap">{{ formatTime(job.updatedAt) }}</TableCell>
                <TableCell>
                  {{ jobKindLabel[job.kind] ?? job.kind }}
                  <a v-if="job.postId" :href="`/post/${job.postId}`" target="_blank" class="ml-1 text-xs underline">帖子</a>
                </TableCell>
                <TableCell>
                  <Badge :variant="jobVariant(job.status)">{{ jobStatusLabel[job.status] ?? job.status }}</Badge>
                  <span v-if="job.attempts > 1" class="ml-1 text-xs text-muted-foreground">第 {{ job.attempts }} 次</span>
                </TableCell>
                <TableCell class="hidden md:table-cell font-mono text-xs">{{ job.modelKey ?? '—' }}</TableCell>
                <TableCell class="hidden md:table-cell text-right tabular-nums">{{ formatNumber(job.promptTokens) }} / {{ formatNumber(job.completionTokens) }}</TableCell>
                <TableCell class="hidden lg:table-cell font-mono text-xs">{{ job.userId }} → {{ job.aiUserId }}</TableCell>
                <TableCell class="max-w-xs truncate text-xs text-muted-foreground" :title="job.reason ?? ''">{{ job.reason ?? '' }}</TableCell>
              </TableRow>
              <TableRow v-if="jobs && jobs.items.length === 0">
                <TableCell colspan="7" class="py-10 text-center text-muted-foreground">没有任务</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
        <AdminPager v-if="jobs" :page="jobs.page" :limit="jobs.limit" :total="jobs.total" unit="个任务" @change="(p) => { jobPage = p; reloadJobs() }" />
      </TabsContent>
    </Tabs>

    <!-- 模型编辑 -->
    <Dialog :open="!!modelDraft" @update:open="(v) => { if (!v) modelDraft = null }">
      <DialogScrollContent v-if="modelDraft" class="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ modelDraft.isNew ? '新建模型' : `编辑模型 ${modelDraft.key}` }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">标识（创建后不可改）</span>
              <Input v-model="modelDraft.key" :readonly="!modelDraft.isNew" class="font-mono" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">显示名称</span>
              <Input v-model="modelDraft.displayName" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">提供方</span>
              <Input v-model="modelDraft.provider" list="ai-providers" class="font-mono" placeholder="部署配置里的提供方名" />
              <datalist id="ai-providers">
                <option v-for="p in settings?.providers ?? []" :key="p" :value="p" />
              </datalist>
              <span v-if="modelDraft.provider && !settings?.providers.includes(modelDraft.provider)" class="text-xs text-warning">
                部署配置里没有这个提供方，调用时会退回默认模型
              </span>
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">上游模型名</span>
              <Input v-model="modelDraft.providerModel" class="font-mono" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">成本权重</span>
              <Input v-model.number="modelDraft.costWeight" type="number" min="0" step="0.1" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">角标（选填）</span>
              <Input v-model="modelDraft.badge" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">排序</span>
              <Input v-model.number="modelDraft.sortOrder" type="number" />
            </label>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">说明</span>
            <Textarea v-model="modelDraft.description" rows="2" />
          </label>
          <label class="flex items-center justify-between gap-4 text-sm">
            启用
            <Switch v-model="modelDraft.enabled" />
          </label>
          <label class="flex items-center justify-between gap-4 text-sm">
            设为默认模型（同时取消其他模型的默认）
            <Switch v-model="modelDraft.isDefault" />
          </label>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="modelDraft = null">取消</Button>
          <Button :disabled="!modelDraft.key.trim()" @click="saveModel">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>

    <!-- 角色编辑 -->
    <Dialog :open="!!characterDraft" @update:open="(v) => { if (!v) characterDraft = null }">
      <DialogScrollContent v-if="characterDraft" class="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{{ characterDraft.isNew ? '新建角色' : `编辑角色 ${characterDraft.slug}` }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="grid gap-4 sm:grid-cols-3">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">标识（创建后不可改）</span>
              <Input v-model="characterDraft.slug" :readonly="!characterDraft.isNew" class="font-mono" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">显示名称</span>
              <Input v-model="characterDraft.displayName" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">绑定账号（用户名）</span>
              <Input v-model="characterDraft.username" placeholder="保存时会标为 AI 账号" />
            </label>
          </div>
          <div class="space-y-2">
            <Label>头像</Label>
            <AdminImagePicker v-model="characterDraft.avatar" />
            <div v-if="characterDraft.avatarId && !characterDraft.avatar" class="flex items-center gap-2 text-xs text-muted-foreground">
              当前头像文件 ID {{ characterDraft.avatarId }}
              <Button size="sm" variant="ghost" @click="characterDraft.avatarId = null">移除</Button>
            </div>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">简介</span>
            <Textarea v-model="characterDraft.bio" rows="2" />
          </label>
          <div class="grid gap-4 sm:grid-cols-3">
            <div class="space-y-2">
              <Label>输出格式</Label>
              <Select v-model="characterDraft.outputMode">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="markdown">Markdown</SelectItem>
                  <SelectItem value="plain">纯文本</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div class="space-y-2">
              <Label>默认模型</Label>
              <Select v-model="characterDraft.defaultModelKey">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">跟随全站默认</SelectItem>
                  <SelectItem v-for="m in models ?? []" :key="m.key" :value="m.key">{{ m.displayName }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">排序</span>
              <Input v-model.number="characterDraft.sortOrder" type="number" />
            </label>
          </div>
          <div class="space-y-2">
            <Label>可选模型（都不勾 = 全部启用的模型）</Label>
            <div class="flex flex-wrap gap-4">
              <label v-for="m in models ?? []" :key="m.key" class="flex items-center gap-2 text-sm">
                <Checkbox :model-value="characterDraft.allowedModelKeys.includes(m.key)" @update:model-value="(v) => toggleAllowed(m.key, v)" />
                {{ m.displayName }}
              </label>
            </div>
          </div>
          <div class="grid gap-3 sm:grid-cols-2">
            <label class="flex items-center justify-between gap-4 text-sm">启用 <Switch v-model="characterDraft.enabled" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">在敲敲列表展示 <Switch v-model="characterDraft.knockVisible" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">允许私聊 <Switch v-model="characterDraft.dmEnabled" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">允许 @ 评论回复 <Switch v-model="characterDraft.commentEnabled" /></label>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">系统提示词</span>
            <Textarea v-model="characterDraft.systemPrompt" rows="10" class="font-mono text-xs" />
          </label>
          <div class="space-y-2">
            <Label>示例对话</Label>
            <div v-for="(example, index) in characterDraft.examples" :key="index" class="grid gap-2 rounded-md border p-3 sm:grid-cols-[1fr_1fr_auto]">
              <Textarea v-model="example.user" rows="2" placeholder="用户说" />
              <Textarea v-model="example.assistant" rows="2" placeholder="角色回" />
              <Button size="icon" variant="ghost" aria-label="删除示例" @click="characterDraft.examples.splice(index, 1)">
                <X class="size-4" />
              </Button>
            </div>
            <Button size="sm" variant="outline" @click="characterDraft.examples.push({ user: '', assistant: '' })">
              <Plus class="size-4" />添加示例
            </Button>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">推荐问题（每行一个）</span>
            <Textarea v-model="characterDraft.suggestedQuestions" rows="3" />
          </label>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="characterDraft = null">取消</Button>
          <Button :disabled="!characterDraft.slug.trim() || !characterDraft.username.trim()" @click="saveCharacter">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
