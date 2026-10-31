<script setup lang="ts">
import { Download, FileText, Plus, RefreshCw, X } from '@lucide/vue'
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
import { formatNumber, formatTime, jobStatusVariant } from '~/lib/format'

definePageMeta({ admin: true })

type Provider = Schemas['AdminProvider']
type Model = Schemas['AdminModel']
type Character = Schemas['AdminCharacter']
type McpServer = Schemas['AdminMcpServer']
type McpTool = Schemas['AdminMcpTool']

const siteLink = useSiteLink()
const { run, success, failure } = useNotify()
const confirm = useConfirm()
const tab = useQueryParam<string>('tab', 'settings')
const loading = ref(false)

const settings = ref<Schemas['AdminSettings'] | null>(null)
const providers = ref<Provider[] | null>(null)
const models = ref<Model[] | null>(null)
const characters = ref<Character[] | null>(null)
const mcpServers = ref<McpServer[] | null>(null)

async function loadConfig() {
  const [s, p, m, c, x] = await Promise.all([
    unwrap(api.GET('/v1/admin/ai/settings')),
    unwrap(api.GET('/v1/admin/ai/providers')),
    unwrap(api.GET('/v1/admin/ai/models')),
    unwrap(api.GET('/v1/admin/ai/characters')),
    unwrap(api.GET('/v1/admin/ai/mcp-servers')),
  ])
  settings.value = s
  providers.value = p
  settingsForm.value = { globalEnabled: s.globalEnabled, dailyTokens: s.dailyTokens, dailyTokensPerUser: s.dailyTokensPerUser, traceEnabled: s.traceEnabled }
  models.value = m
  characters.value = c
  mcpServers.value = x
}

async function load() {
  loading.value = true
  try {
    await loadConfig()
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

function refresh() {
  load()
  jobList.refresh()
  badges.refresh()
}

// ---------- 全局设置 ----------
const settingsForm = ref<Schemas['SettingsInput']>({ globalEnabled: false, dailyTokens: 0, dailyTokensPerUser: 0, traceEnabled: false })

async function saveSettings() {
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/settings', { body: settingsForm.value })), '设置已保存')
  if (saved) settings.value = saved
}

// ---------- 提供方 ----------
interface ProviderDraft {
  isNew: boolean
  name: string
  displayName: string
  baseUrl: string
  /** 留空 = 保留原密钥（新建时即不带密钥）。 */
  apiKey: string
  clearApiKey: boolean
  apiKeyHint: string | null
  enabled: boolean
  sortOrder: number
}

const providerDraft = ref<ProviderDraft | null>(null)
/** 每个提供方最近一次测试连接列出的上游模型（填写上游模型名时提示）。 */
const upstreamModels = ref<Record<string, string[]>>({})
const testing = ref<string | null>(null)

function editProvider(provider?: Provider) {
  providerDraft.value = provider
    ? {
        isNew: false,
        name: provider.name,
        displayName: provider.displayName,
        baseUrl: provider.baseUrl,
        apiKey: '',
        clearApiKey: false,
        apiKeyHint: provider.apiKeyHint ?? null,
        enabled: provider.enabled,
        sortOrder: provider.sortOrder,
      }
    : {
        isNew: true,
        name: '',
        displayName: '',
        baseUrl: '',
        apiKey: '',
        clearApiKey: false,
        apiKeyHint: null,
        enabled: true,
        sortOrder: (providers.value?.length ?? 0) * 10,
      }
}

async function saveProvider() {
  const d = providerDraft.value
  if (!d) return
  const apiKey = d.clearApiKey ? '' : d.apiKey.trim() || (d.isNew ? '' : null)
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/providers/{name}', {
    params: { path: { name: d.name } },
    body: {
      name: d.name,
      displayName: d.displayName.trim() || d.name,
      baseUrl: d.baseUrl,
      apiKey,
      enabled: d.enabled,
      sortOrder: d.sortOrder,
    },
  })), '提供方已保存')
  if (saved) {
    providerDraft.value = null
    await loadConfig().catch(failure)
  }
}

async function removeProvider(provider: Provider) {
  const ok = await confirm({
    title: `删除提供方「${provider.displayName}」？`,
    description: '还有模型在用它时不能删除，请先把这些模型改到别的提供方。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/ai/providers/{name}', { params: { path: { name: provider.name } } })).then(() => true), '已删除')
  if (done) await loadConfig().catch(failure)
}

/** 用已保存的地址与密钥请求上游的模型列表。 */
async function testProvider(name: string, quiet = false) {
  testing.value = name
  try {
    const result = await unwrap(api.POST('/v1/admin/ai/providers/{name}/test', { params: { path: { name } } }))
    if (result.ok) {
      upstreamModels.value = { ...upstreamModels.value, [name]: result.models }
      if (!quiet) success(`连接成功，上游有 ${result.models.length} 个模型（${result.elapsedMs} ms）`)
    }
    else {
      failure(new Error(`连接失败：${result.error ?? '未知错误'}`))
    }
  }
  catch (error) {
    failure(error)
  }
  finally {
    testing.value = null
  }
}

const providerSourceLabel: Record<Schemas['ProviderSource'], string> = { admin: '后台', config: '部署配置' }

// ---------- 模型 ----------
/** 数字参数在表单里是字符串：留空 = 不填。 */
interface ParamsDraft {
  temperature: string
  topP: string
  maxOutputTokens: string
  contextTokens: string
  cachedWeight: string
  extraBody: string
}

interface ModelDraft extends Required<Omit<Schemas['ModelInput'], 'badge' | 'params'>> {
  badge: string
  params: ParamsDraft
  isNew: boolean
}

const text = (value?: number | null) => (value == null ? '' : String(value))
// type="number" 的输入框会把值转成数字，空串保持空串。
const numberOrNull = (value: string | number) => (String(value).trim() === '' ? null : Number(value))

function paramsDraft(params?: Schemas['ModelParams']): ParamsDraft {
  return {
    temperature: text(params?.temperature),
    topP: text(params?.topP),
    maxOutputTokens: text(params?.maxOutputTokens),
    contextTokens: text(params?.contextTokens),
    cachedWeight: text(params?.cachedWeight),
    extraBody: params?.extraBody && Object.keys(params.extraBody).length ? JSON.stringify(params.extraBody, null, 2) : '',
  }
}

/** 附加参数不是 JSON 对象时返回 null。 */
function modelParams(d: ParamsDraft): Schemas['ModelParams'] | null {
  let extraBody: unknown = {}
  if (d.extraBody.trim()) {
    try {
      extraBody = JSON.parse(d.extraBody)
    }
    catch {
      return null
    }
    if (typeof extraBody !== 'object' || extraBody === null || Array.isArray(extraBody)) return null
  }
  return {
    temperature: numberOrNull(d.temperature),
    topP: numberOrNull(d.topP),
    maxOutputTokens: numberOrNull(d.maxOutputTokens),
    contextTokens: numberOrNull(d.contextTokens),
    cachedWeight: numberOrNull(d.cachedWeight),
    extraBody: extraBody as Schemas['ModelParams']['extraBody'],
  }
}

const modelDraft = ref<ModelDraft | null>(null)

function editModel(model?: Model) {
  // 逐个取保存接口认识的字段：列表里的 providerConfigured 等只读字段不能带回去，
  // 保存接口拒绝未知字段（deny_unknown_fields）。
  modelDraft.value = model
    ? {
        key: model.key,
        displayName: model.displayName,
        description: model.description,
        badge: model.badge ?? '',
        provider: model.provider,
        providerModel: model.providerModel,
        costWeight: model.costWeight,
        vision: model.vision,
        enabled: model.enabled,
        isDefault: model.isDefault,
        sortOrder: model.sortOrder,
        params: paramsDraft(model.params),
        isNew: false,
      }
    : {
        key: '',
        displayName: '',
        description: '',
        badge: '',
        provider: settings.value?.providers[0] ?? '',
        providerModel: '',
        costWeight: 1,
        vision: false,
        enabled: true,
        isDefault: false,
        sortOrder: (models.value?.length ?? 0) * 10,
        params: paramsDraft(),
        isNew: true,
      }
}

async function saveModel() {
  const draft = modelDraft.value
  if (!draft) return
  const { isNew: _isNew, badge, params: paramsText, ...rest } = draft
  const params = modelParams(paramsText)
  if (!params) {
    failure(new Error('附加请求参数要是 JSON 对象，例如 {"enable_thinking": false}'))
    return
  }
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/models/{key}', {
    params: { path: { key: draft.key } },
    body: { ...rest, badge: badge.trim() || null, params },
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
  qqEnabled: boolean
  sortOrder: number
  outputMode: Schemas['OutputMode']
  systemPrompt: string
  examples: Schemas['Example'][]
  defaultModelKey: string
  allowedModelKeys: string[]
  suggestedQuestions: string
  storyTools: boolean
  mcpServers: string[]
  temperature: string
  topP: string
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
        qqEnabled: character.qqEnabled,
        sortOrder: character.sortOrder,
        outputMode: character.outputMode,
        systemPrompt: character.systemPrompt,
        examples: character.examples.map(e => ({ ...e })),
        defaultModelKey: character.defaultModelKey ?? 'none',
        allowedModelKeys: [...character.allowedModelKeys],
        suggestedQuestions: character.suggestedQuestions.join('\n'),
        storyTools: character.storyTools,
        mcpServers: [...character.mcpServers],
        temperature: text(character.params.temperature),
        topP: text(character.params.topP),
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
        qqEnabled: false,
        sortOrder: (characters.value?.length ?? 0) * 10,
        outputMode: 'markdown',
        systemPrompt: '',
        examples: [],
        defaultModelKey: 'none',
        allowedModelKeys: [],
        suggestedQuestions: '',
        storyTools: false,
        mcpServers: [],
        temperature: '',
        topP: '',
      }
}

function toggleAllowed(key: string, on: boolean | 'indeterminate') {
  const draft = characterDraft.value
  if (!draft) return
  draft.allowedModelKeys = on === true
    ? [...new Set([...draft.allowedModelKeys, key])]
    : draft.allowedModelKeys.filter(k => k !== key)
}

function toggleMcpServer(name: string, on: boolean | 'indeterminate') {
  const draft = characterDraft.value
  if (!draft) return
  draft.mcpServers = on === true
    ? [...new Set([...draft.mcpServers, name])]
    : draft.mcpServers.filter(n => n !== name)
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
    qqEnabled: d.qqEnabled,
    sortOrder: d.sortOrder,
    outputMode: d.outputMode,
    systemPrompt: d.systemPrompt,
    examples: d.examples.filter(e => e.user.trim() || e.assistant.trim()),
    defaultModelKey: d.defaultModelKey === 'none' ? null : d.defaultModelKey,
    allowedModelKeys: d.allowedModelKeys,
    suggestedQuestions: d.suggestedQuestions.split('\n').map(q => q.trim()).filter(Boolean),
    storyTools: d.storyTools,
    mcpServers: d.mcpServers,
    params: { temperature: numberOrNull(d.temperature), topP: numberOrNull(d.topP) },
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

// ---------- MCP 服务 ----------
interface McpServerDraft {
  isNew: boolean
  name: string
  displayName: string
  url: string
  /** 留空 = 保留原有的（新建时即不带）。 */
  authHeader: string
  clearAuthHeader: boolean
  authHeaderHint: string | null
  timeoutMs: number
  enabled: boolean
  sortOrder: number
}

const mcpDraft = ref<McpServerDraft | null>(null)
const mcpBusy = ref<string | null>(null)

function editMcpServer(server?: McpServer) {
  mcpDraft.value = server
    ? {
        isNew: false,
        name: server.name,
        displayName: server.displayName,
        url: server.url,
        authHeader: '',
        clearAuthHeader: false,
        authHeaderHint: server.authHeaderHint ?? null,
        timeoutMs: server.timeoutMs,
        enabled: server.enabled,
        sortOrder: server.sortOrder,
      }
    : {
        isNew: true,
        name: '',
        displayName: '',
        url: '',
        authHeader: '',
        clearAuthHeader: false,
        authHeaderHint: null,
        timeoutMs: 15000,
        enabled: true,
        sortOrder: (mcpServers.value?.length ?? 0) * 10,
      }
}

async function saveMcpServer() {
  const d = mcpDraft.value
  if (!d) return
  const authHeader = d.clearAuthHeader ? '' : d.authHeader.trim() || (d.isNew ? '' : null)
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/mcp-servers/{name}', {
    params: { path: { name: d.name } },
    body: {
      name: d.name,
      displayName: d.displayName.trim() || d.name,
      url: d.url.trim(),
      authHeader,
      timeoutMs: d.timeoutMs,
      enabled: d.enabled,
      sortOrder: d.sortOrder,
    },
  })), 'MCP 服务已保存')
  if (saved) {
    const isNew = d.isNew
    mcpDraft.value = null
    await loadConfig().catch(failure)
    if (isNew) await refreshMcpServer(saved.name)
  }
}

async function removeMcpServer(server: McpServer) {
  const ok = await confirm({
    title: `删除 MCP 服务「${server.displayName}」？`,
    description: '它的工具会一起删除，启用了它的角色不再提供这些工具。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/ai/mcp-servers/{name}', { params: { path: { name: server.name } } })).then(() => true), '已删除')
  if (done) await loadConfig().catch(failure)
}

/** 拉取工具清单：新工具默认不启用，定义变了的停止提供、等重新确认。 */
async function refreshMcpServer(name: string) {
  mcpBusy.value = name
  try {
    const result = await unwrap(api.POST('/v1/admin/ai/mcp-servers/{name}/refresh', { params: { path: { name } } }))
    replaceMcpServer(result.server)
    if (!result.ok) {
      failure(new Error(`拉取失败：${result.error ?? '未知错误'}`))
      return
    }
    const parts = [
      result.added.length ? `新增 ${result.added.length} 个（未启用）` : '',
      result.changed.length ? `${result.changed.length} 个定义变了，需要重新确认` : '',
      result.missing.length ? `${result.missing.length} 个已从远端移除` : '',
      result.skipped.length ? `跳过 ${result.skipped.join('；')}` : '',
    ].filter(Boolean)
    success(`已拉取 ${result.server.tools.length} 个工具${parts.length ? `：${parts.join('，')}` : ''}`)
  }
  catch (error) {
    failure(error)
  }
  finally {
    mcpBusy.value = null
  }
}

function replaceMcpServer(server: McpServer) {
  if (!mcpServers.value) return
  mcpServers.value = mcpServers.value.map(s => (s.name === server.name ? server : s))
}

async function updateMcpTool(server: McpServer, tool: McpTool, enabled: boolean, timelineText = tool.timelineText ?? null) {
  const saved = await run(() => unwrap(api.PUT('/v1/admin/ai/mcp-servers/{name}/tools/{tool}', {
    params: { path: { name: server.name, tool: tool.remoteName } },
    body: { enabled, digest: enabled ? tool.digest : null, timelineText },
  })), enabled ? '工具已启用' : '已保存')
  if (saved) {
    replaceMcpServer({ ...server, tools: server.tools.map(t => (t.remoteName === saved.remoteName ? saved : t)) })
  }
}

/** 启用（或确认新版本）前把模型会看到的描述给管理员过目。 */
async function approveMcpTool(server: McpServer, tool: McpTool) {
  const ok = await confirm({
    title: `${tool.status === 'changed' ? '确认新版本' : '启用'}「${tool.title ?? tool.remoteName}」？`,
    description: `模型会看到这段描述：${tool.description || '（无描述）'}`,
    confirmLabel: tool.status === 'changed' ? '确认' : '启用',
  })
  if (ok) await updateMcpTool(server, tool, true)
}

/** 改时间线文案：不改变启用状态，也不会确认没看过的新版本。 */
async function saveMcpTimeline(server: McpServer, tool: McpTool, value: string | number) {
  const text = String(value).trim() || null
  if (text === (tool.timelineText ?? null)) return
  await updateMcpTool(server, tool, tool.status === 'active', text)
}

const mcpToolStatusLabel: Record<Schemas['McpToolStatus'], string> = {
  active: '已启用',
  disabled: '未启用',
  changed: '定义已变，待确认',
  missing: '远端已移除',
}
const mcpToolStatusVariant: Record<Schemas['McpToolStatus'], 'default' | 'outline' | 'destructive' | 'secondary'> = {
  active: 'default',
  disabled: 'outline',
  changed: 'destructive',
  missing: 'secondary',
}

// ---------- 任务 ----------
const badges = useBadges()
const jobStatusLabel: Record<string, string> = { queued: '排队', processing: '进行中', completed: '完成', failed: '失败', skipped: '跳过' }
const jobKindLabel: Record<string, string> = { dm: '私聊', comment: '评论' }

const jobList = useListQuery({
  filters: { jobStatus: 'all', jobKind: 'all' },
  pageKey: 'jobPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/ai/jobs', {
    params: {
      query: {
        status: optional(f.jobStatus),
        kind: optional(f.jobKind),
        page,
        limit: 50,
      },
    },
  })),
})
const jobs = jobList.result
const jobFilters = jobList.filters

const ms = (value?: number | null) => (value == null ? '—' : value < 1000 ? `${value} ms` : `${(value / 1000).toFixed(1)} s`)

const trace = ref<{ job: Schemas['AdminJob'], items: Schemas['AdminJobTrace'][] } | null>(null)

async function openTrace(job: Schemas['AdminJob']) {
  const items = await run(() => unwrap(api.GET('/v1/admin/ai/jobs/{id}/trace', { params: { path: { id: job.id } } })))
  if (items) trace.value = { job, items }
}

/** 请求里的消息：角色 + 正文（工具调用写成一行）。 */
function traceMessages(request: unknown) {
  const messages = (request as { messages?: Record<string, unknown>[] })?.messages ?? []
  return messages.map((m) => {
    const content = m.content
    const body = typeof content === 'string'
      ? content
      : Array.isArray(content)
        ? content.map(part => (part as { text?: string }).text ?? '[图片]').join('\n')
        : ''
    const calls = (m.tool_calls as { function?: { name?: string, arguments?: string } }[] | undefined)
      ?.map(c => `→ ${c.function?.name}(${c.function?.arguments ?? ''})`)
      .join('\n')
    return { role: String(m.role ?? ''), text: [body, calls].filter(Boolean).join('\n') }
  })
}
</script>

<template>
  <div>
    <AdminPageHeader title="AI 助手" description="模型、角色、额度与工具。">
      <AdminRefreshButton :loading="loading || jobList.loading.value" @click="refresh" />
      <Button v-if="tab === 'providers'" size="sm" @click="editProvider()">
        <Plus class="size-4" />新建提供方
      </Button>
      <Button v-if="tab === 'models'" size="sm" @click="editModel()">
        <Plus class="size-4" />新建模型
      </Button>
      <Button v-if="tab === 'characters'" size="sm" @click="editCharacter()">
        <Plus class="size-4" />新建角色
      </Button>
      <Button v-if="tab === 'mcp'" size="sm" @click="editMcpServer()">
        <Plus class="size-4" />接入 MCP 服务
      </Button>
    </AdminPageHeader>

    <Tabs v-model="tab">
      <TabsList class="mb-4">
        <TabsTrigger value="settings">全局设置</TabsTrigger>
        <TabsTrigger value="providers">提供方</TabsTrigger>
        <TabsTrigger value="models">模型</TabsTrigger>
        <TabsTrigger value="characters">角色</TabsTrigger>
        <TabsTrigger value="mcp">MCP 工具</TabsTrigger>
        <TabsTrigger value="jobs">任务</TabsTrigger>
      </TabsList>

      <TabsContent value="settings">
        <Card class="max-w-xl">
          <CardHeader>
            <CardTitle>开关与额度</CardTitle>
            <CardDescription>token 按模型成本权重折算，0 为不限。</CardDescription>
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
            <label class="flex items-center justify-between gap-4 text-sm">
              <span>
                保存调用记录
                <span class="block text-xs text-muted-foreground">在「任务」里查看发给模型的内容，只留 7 天。会存下私聊原文，排查完记得关。</span>
              </span>
              <Switch v-model="settingsForm.traceEnabled" />
            </label>
            <div class="text-sm text-muted-foreground">
              可用的提供方：
              <template v-if="settings?.providers.length">
                <Badge v-for="p in settings.providers" :key="p" variant="outline" class="mr-1">{{ p }}</Badge>
              </template>
              <span v-else>无（在「提供方」里添加）</span>
            </div>
            <Button @click="saveSettings">保存</Button>
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="providers">
        <p class="mb-4 text-sm text-muted-foreground">
          支持任意 OpenAI 兼容接口。部署配置里的提供方在这里只读。
        </p>
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>提供方</TableHead>
                <TableHead class="hidden md:table-cell">接口地址</TableHead>
                <TableHead class="hidden sm:table-cell">密钥</TableHead>
                <TableHead class="hidden lg:table-cell">模型</TableHead>
                <TableHead>状态</TableHead>
                <TableHead class="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="provider in providers ?? []" :key="`${provider.source}:${provider.name}`">
                <TableCell>
                  <div class="font-medium">{{ provider.displayName }}</div>
                  <div class="font-mono text-xs text-muted-foreground">{{ provider.name }}</div>
                </TableCell>
                <TableCell class="hidden max-w-xs truncate font-mono text-xs md:table-cell" :title="provider.baseUrl">{{ provider.baseUrl || '—' }}</TableCell>
                <TableCell class="hidden font-mono text-xs sm:table-cell">{{ provider.apiKeyHint ?? '无' }}</TableCell>
                <TableCell class="hidden lg:table-cell">
                  <span v-if="provider.modelKeys.length" class="font-mono text-xs">{{ provider.modelKeys.join('、') }}</span>
                  <span v-else class="text-xs text-muted-foreground">未使用</span>
                </TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge :variant="provider.source === 'config' ? 'secondary' : 'outline'">{{ providerSourceLabel[provider.source] }}</Badge>
                    <Badge v-if="!provider.enabled" variant="outline">停用</Badge>
                    <Badge v-if="provider.shadowed" variant="destructive" title="部署配置里有同名的提供方，这一条不生效">被部署配置覆盖</Badge>
                  </div>
                </TableCell>
                <TableCell class="space-x-1 text-right whitespace-nowrap">
                  <Button size="sm" variant="outline" :disabled="testing === provider.name" @click="testProvider(provider.name)">
                    {{ testing === provider.name ? '测试中…' : '测试' }}
                  </Button>
                  <template v-if="provider.source === 'admin'">
                    <Button size="sm" variant="outline" @click="editProvider(provider)">编辑</Button>
                    <Button size="sm" variant="ghost" class="text-destructive" @click="removeProvider(provider)">删除</Button>
                  </template>
                </TableCell>
              </TableRow>
              <TableRow v-if="providers && providers.length === 0">
                <TableCell colspan="6" class="py-10 text-center text-muted-foreground">还没有提供方</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </TabsContent>

      <TabsContent value="models">
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
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
                    <Badge v-if="model.vision" variant="secondary">识图</Badge>
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
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
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
                    <Badge v-if="character.qqEnabled" variant="outline">QQ</Badge>
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

      <TabsContent value="mcp">
        <div class="mb-4 space-y-2 text-sm text-muted-foreground">
          <p>
            接入站外 MCP 服务后，工具需要逐个启用，再到「角色」里开启。
          </p>
          <p>
            只能启用只读工具。参数里可能带有对话内容，请只接入可信的服务。
          </p>
          <p>
            剧情库：
            <Badge :variant="settings?.storyAvailable ? 'default' : 'outline'">{{ settings?.storyAvailable ? '语料已加载' : '没有语料，不会提供' }}</Badge>
          </p>
        </div>
        <div class="space-y-4">
          <Card v-for="server in mcpServers ?? []" :key="server.name">
            <CardHeader>
              <div class="flex flex-wrap items-start justify-between gap-2">
                <div class="min-w-0">
                  <CardTitle class="flex flex-wrap items-center gap-2">
                    {{ server.displayName }}
                    <span class="font-mono text-xs font-normal text-muted-foreground">{{ server.name }}__*</span>
                    <Badge v-if="!server.enabled" variant="outline">停用</Badge>
                  </CardTitle>
                  <CardDescription class="mt-1 space-y-1">
                    <div class="truncate font-mono text-xs" :title="server.url">{{ server.url }}</div>
                    <div class="text-xs">
                      鉴权头：<span class="font-mono">{{ server.authHeaderHint ?? '无' }}</span>
                      · 超时 {{ server.timeoutMs / 1000 }} 秒
                      · 角色：{{ server.characters.length ? server.characters.join('、') : '未启用' }}
                    </div>
                    <div class="text-xs">
                      最近拉取：{{ server.lastListedAt ? formatTime(server.lastListedAt) : '从未' }}
                      <span v-if="server.lastError" class="text-destructive">（失败：{{ server.lastError }}）</span>
                    </div>
                  </CardDescription>
                </div>
                <div class="space-x-1 whitespace-nowrap">
                  <Button size="sm" variant="outline" :disabled="mcpBusy === server.name" @click="refreshMcpServer(server.name)">
                    <RefreshCw class="size-4" :class="{ 'animate-spin': mcpBusy === server.name }" />拉取工具
                  </Button>
                  <Button size="sm" variant="outline" @click="editMcpServer(server)">编辑</Button>
                  <Button size="sm" variant="ghost" class="text-destructive" @click="removeMcpServer(server)">删除</Button>
                </div>
              </div>
            </CardHeader>
            <CardContent>
              <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>工具</TableHead>
                      <TableHead class="hidden md:table-cell">描述</TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead class="hidden sm:table-cell">时间线文案</TableHead>
                      <TableHead class="text-right">操作</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow v-for="tool in server.tools" :key="tool.remoteName">
                      <TableCell>
                        <div class="font-medium">{{ tool.title ?? tool.remoteName }}</div>
                        <div class="font-mono text-xs text-muted-foreground">{{ tool.exposedName }}</div>
                      </TableCell>
                      <TableCell class="hidden max-w-sm md:table-cell">
                        <div class="line-clamp-2 text-xs text-muted-foreground" :title="tool.description">{{ tool.description || '—' }}</div>
                      </TableCell>
                      <TableCell>
                        <div class="flex flex-wrap gap-1">
                          <Badge :variant="mcpToolStatusVariant[tool.status]">{{ mcpToolStatusLabel[tool.status] }}</Badge>
                          <Badge v-if="!tool.readOnly" variant="outline" title="没有声明 readOnlyHint，不能启用">非只读</Badge>
                        </div>
                      </TableCell>
                      <TableCell class="hidden sm:table-cell">
                        <Input
                          :model-value="tool.timelineText ?? ''" class="h-8 w-40 text-xs" maxlength="32"
                          :placeholder="tool.title ?? '如 检索资料库'" :disabled="tool.status === 'changed' || tool.status === 'missing'"
                          @change="(e: Event) => saveMcpTimeline(server, tool, (e.target as HTMLInputElement).value)"
                        />
                      </TableCell>
                      <TableCell class="space-x-1 text-right whitespace-nowrap">
                        <Button
                          v-if="tool.status === 'disabled' || tool.status === 'changed'" size="sm" variant="outline"
                          :disabled="!tool.readOnly" @click="approveMcpTool(server, tool)"
                        >
                          {{ tool.status === 'changed' ? '确认新版本' : '启用' }}
                        </Button>
                        <Button v-if="tool.enabled" size="sm" variant="ghost" @click="updateMcpTool(server, tool, false)">停用</Button>
                      </TableCell>
                    </TableRow>
                    <TableRow v-if="server.tools.length === 0">
                      <TableCell colspan="5" class="py-6 text-center text-muted-foreground">还没有工具，点「拉取工具」</TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </div>
            </CardContent>
          </Card>
          <div v-if="mcpServers && mcpServers.length === 0" class="rounded-md border py-10 text-center text-sm text-muted-foreground">
            还没有接入 MCP 服务
          </div>
        </div>
      </TabsContent>

      <TabsContent value="jobs">
        <div class="mb-4 grid gap-2 sm:grid-cols-4">
          <Select v-model="jobFilters.jobStatus">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部状态</SelectItem>
              <SelectItem v-for="(label, key) in jobStatusLabel" :key="key" :value="key">{{ label }}</SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="jobFilters.jobKind">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部类型</SelectItem>
              <SelectItem v-for="(label, key) in jobKindLabel" :key="key" :value="key">{{ label }}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>时间</TableHead>
                <TableHead>类型</TableHead>
                <TableHead>状态</TableHead>
                <TableHead class="hidden md:table-cell">模型</TableHead>
                <TableHead class="hidden md:table-cell text-right">token（入 / 出）</TableHead>
                <TableHead class="hidden lg:table-cell text-right">首字 / 总耗时</TableHead>
                <TableHead class="hidden lg:table-cell">用户 / AI 账号</TableHead>
                <TableHead>原因</TableHead>
                <TableHead class="w-10" />
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="job in jobs?.items ?? []" :key="job.id">
                <TableCell class="whitespace-nowrap">{{ formatTime(job.updatedAt) }}</TableCell>
                <TableCell>
                  {{ jobKindLabel[job.kind] ?? job.kind }}
                  <a v-if="job.postId" :href="siteLink(`/post/${job.postId}`)" target="_blank" class="ml-1 text-xs underline">帖子</a>
                </TableCell>
                <TableCell>
                  <Badge :variant="jobStatusVariant(job.status)">{{ jobStatusLabel[job.status] ?? job.status }}</Badge>
                  <span v-if="job.attempts > 1" class="ml-1 text-xs text-muted-foreground">第 {{ job.attempts }} 次</span>
                </TableCell>
                <TableCell class="hidden md:table-cell font-mono text-xs">{{ job.modelKey ?? '—' }}</TableCell>
                <TableCell class="hidden md:table-cell text-right tabular-nums">
                  {{ formatNumber(job.promptTokens) }} / {{ formatNumber(job.completionTokens) }}
                  <span v-if="job.cachedTokens" class="block text-xs text-muted-foreground">缓存 {{ formatNumber(job.cachedTokens) }}</span>
                </TableCell>
                <TableCell class="hidden lg:table-cell text-right tabular-nums">
                  {{ ms(job.firstDeltaMs) }} / {{ ms(job.elapsedMs) }}
                  <span v-if="job.toolCalls || job.retries" class="block text-xs text-muted-foreground">
                    <template v-if="job.toolCalls">工具 {{ job.toolCalls }} 次</template>
                    <template v-if="job.toolCalls && job.retries">，</template>
                    <template v-if="job.retries">重发 {{ job.retries }} 次</template>
                  </span>
                </TableCell>
                <TableCell class="hidden lg:table-cell font-mono text-xs">{{ job.userId }} → {{ job.aiUserId }}</TableCell>
                <TableCell class="max-w-xs truncate text-xs text-muted-foreground" :title="job.reason ?? ''">{{ job.reason ?? '' }}</TableCell>
                <TableCell>
                  <Button v-if="job.hasTrace" size="icon" variant="ghost" aria-label="调用记录" @click="openTrace(job)">
                    <FileText class="size-4" />
                  </Button>
                </TableCell>
              </TableRow>
              <AdminListState
                :colspan="9" :loading="jobList.loading.value" :failed="jobList.failed.value" :filtered="jobList.active.value"
                :empty="!jobs?.items.length" empty-text="还没有 AI 任务"
                @retry="jobList.refresh" @reset="jobList.reset"
              />
            </TableBody>
          </Table>
        </div>
        <AdminPager v-if="jobs" :page="jobs.page" :limit="jobs.limit" :total="jobs.total" unit="个任务" @change="jobList.setPage" />
      </TabsContent>
    </Tabs>

    <!-- 提供方编辑 -->
    <Dialog :open="!!providerDraft" @update:open="(v) => { if (!v) providerDraft = null }">
      <DialogScrollContent v-if="providerDraft" class="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ providerDraft.isNew ? '新建提供方' : `编辑提供方 ${providerDraft.name}` }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">标识（创建后不可改）</span>
              <Input v-model="providerDraft.name" :readonly="!providerDraft.isNew" class="font-mono" placeholder="如 deepseek" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">显示名称</span>
              <Input v-model="providerDraft.displayName" placeholder="如 DeepSeek 官方" />
            </label>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">接口地址（Base URL）</span>
            <Input v-model="providerDraft.baseUrl" class="font-mono" placeholder="https://api.deepseek.com/v1" />
            <span class="text-xs text-muted-foreground">不含 /chat/completions。</span>
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">API Key</span>
            <Input
              v-model="providerDraft.apiKey" type="password" autocomplete="off" class="font-mono"
              :disabled="providerDraft.clearApiKey"
              :placeholder="providerDraft.isNew ? 'sk-…（本地部署可留空）' : `留空则保留原密钥${providerDraft.apiKeyHint ? `（${providerDraft.apiKeyHint}）` : ''}`"
            />
          </label>
          <label v-if="!providerDraft.isNew && providerDraft.apiKeyHint" class="flex items-center gap-2 text-sm">
            <Checkbox v-model="providerDraft.clearApiKey" />
            清除密钥（不带密钥请求上游）
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">排序</span>
            <Input v-model.number="providerDraft.sortOrder" type="number" />
          </label>
          <label class="flex items-center justify-between gap-4 text-sm">
            启用
            <Switch v-model="providerDraft.enabled" />
          </label>
          <p class="text-xs text-muted-foreground">保存后可以在列表里测试连接。</p>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="providerDraft = null">取消</Button>
          <Button :disabled="!providerDraft.name.trim() || !providerDraft.baseUrl.trim()" @click="saveProvider">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>

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
              <Input v-model="modelDraft.provider" list="ai-providers" class="font-mono" placeholder="「提供方」里的名字" />
              <datalist id="ai-providers">
                <option v-for="p in settings?.providers ?? []" :key="p" :value="p" />
              </datalist>
              <span v-if="modelDraft.provider && !settings?.providers.includes(modelDraft.provider)" class="text-xs text-warning">
                没有这个提供方或已停用，调用时会退回默认模型
              </span>
            </label>
            <div class="grid gap-2">
              <Label for="ai-provider-model">上游模型名</Label>
              <div class="flex gap-2">
                <Input id="ai-provider-model" v-model="modelDraft.providerModel" list="ai-upstream-models" class="font-mono" />
                <Button
                  size="icon" variant="outline" title="从上游获取模型列表" aria-label="从上游获取模型列表"
                  :disabled="!modelDraft.provider || testing === modelDraft.provider || !providers?.some(p => p.name === modelDraft?.provider)"
                  @click="testProvider(modelDraft.provider, true)"
                >
                  <Download class="size-4" />
                </Button>
              </div>
              <datalist id="ai-upstream-models">
                <option v-for="m in upstreamModels[modelDraft.provider] ?? []" :key="m" :value="m" />
              </datalist>
              <span v-if="upstreamModels[modelDraft.provider]" class="text-xs text-muted-foreground">
                上游有 {{ upstreamModels[modelDraft.provider]?.length }} 个模型，输入时可选
              </span>
            </div>
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
            支持识图
            <Switch v-model="modelDraft.vision" />
          </label>
          <label class="flex items-center justify-between gap-4 text-sm">
            设为默认模型（同时取消其他模型的默认）
            <Switch v-model="modelDraft.isDefault" />
          </label>
          <div class="space-y-2">
            <Label>生成参数（留空用默认）</Label>
            <div class="grid gap-4 sm:grid-cols-3">
              <label class="grid gap-2">
                <span class="text-sm leading-none font-medium">温度</span>
                <Input v-model="modelDraft.params.temperature" type="number" min="0" max="2" step="0.1" placeholder="上游默认" />
              </label>
              <label class="grid gap-2">
                <span class="text-sm leading-none font-medium">top_p</span>
                <Input v-model="modelDraft.params.topP" type="number" min="0" max="1" step="0.05" placeholder="上游默认" />
              </label>
              <label class="grid gap-2">
                <span class="text-sm leading-none font-medium">单次最多输出 token</span>
                <Input v-model="modelDraft.params.maxOutputTokens" type="number" min="16" placeholder="上游默认" />
              </label>
              <label class="grid gap-2">
                <span class="text-sm leading-none font-medium">上下文长度 token</span>
                <Input v-model="modelDraft.params.contextTokens" type="number" min="4096" placeholder="65536" />
              </label>
              <label class="grid gap-2">
                <span class="text-sm leading-none font-medium">缓存命中按几成计</span>
                <Input v-model="modelDraft.params.cachedWeight" type="number" min="0" max="1" step="0.05" placeholder="0.2" />
              </label>
            </div>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">附加请求参数（JSON）</span>
              <Textarea v-model="modelDraft.params.extraBody" rows="2" class="font-mono text-xs" placeholder='{"enable_thinking": false}' />
            </label>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="modelDraft = null">取消</Button>
          <Button :disabled="!modelDraft.key.trim()" @click="saveModel">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>

    <!-- MCP 服务编辑 -->
    <Dialog :open="!!mcpDraft" @update:open="(v) => { if (!v) mcpDraft = null }">
      <DialogScrollContent v-if="mcpDraft" class="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ mcpDraft.isNew ? '接入 MCP 服务' : `编辑 MCP 服务 ${mcpDraft.name}` }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">标识（工具名前缀，创建后不可改）</span>
              <Input v-model="mcpDraft.name" :readonly="!mcpDraft.isNew" class="font-mono" placeholder="如 akasha" maxlength="32" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">显示名称</span>
              <Input v-model="mcpDraft.displayName" placeholder="如 虚空终端" />
            </label>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">端点地址（Streamable HTTP）</span>
            <Input v-model="mcpDraft.url" class="font-mono" placeholder="https://example.com/mcp" />
            <span class="text-xs text-muted-foreground">需要 https://，内网地址可用 http://。</span>
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">鉴权请求头（整行）</span>
            <Input
              v-model="mcpDraft.authHeader" type="password" autocomplete="off" class="font-mono"
              :disabled="mcpDraft.clearAuthHeader"
              :placeholder="mcpDraft.isNew ? 'Authorization: Bearer …（不需要可留空）' : `留空则保留原有的${mcpDraft.authHeaderHint ? `（${mcpDraft.authHeaderHint}）` : ''}`"
            />
          </label>
          <label v-if="!mcpDraft.isNew && mcpDraft.authHeaderHint" class="flex items-center gap-2 text-sm">
            <Checkbox v-model="mcpDraft.clearAuthHeader" />
            清除鉴权头
          </label>
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">超时（毫秒）</span>
              <Input v-model.number="mcpDraft.timeoutMs" type="number" min="1000" max="60000" step="1000" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">排序</span>
              <Input v-model.number="mcpDraft.sortOrder" type="number" />
            </label>
          </div>
          <label class="flex items-center justify-between gap-4 text-sm">
            启用
            <Switch v-model="mcpDraft.enabled" />
          </label>
          <p class="text-xs text-muted-foreground">保存后会自动拉取工具，需要逐个启用。</p>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="mcpDraft = null">取消</Button>
          <Button :disabled="!mcpDraft.name.trim() || !mcpDraft.url.trim()" @click="saveMcpServer">保存</Button>
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
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">温度</span>
              <Input v-model="characterDraft.temperature" type="number" min="0" max="2" step="0.1" placeholder="跟随模型" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">top_p</span>
              <Input v-model="characterDraft.topP" type="number" min="0" max="1" step="0.05" placeholder="跟随模型" />
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
          <div class="space-y-2">
            <Label>工具（论坛工具始终提供）</Label>
            <label class="flex items-center justify-between gap-4 text-sm">
              <span>
                剧情库工具（检索、顺读绝区零剧情与设定）
                <span v-if="!settings?.storyAvailable" class="text-xs text-muted-foreground">（暂无语料）</span>
              </span>
              <Switch v-model="characterDraft.storyTools" />
            </label>
            <div v-if="mcpServers?.length" class="flex flex-wrap gap-4">
              <label v-for="s in mcpServers" :key="s.name" class="flex items-center gap-2 text-sm">
                <Checkbox :model-value="characterDraft.mcpServers.includes(s.name)" @update:model-value="(v) => toggleMcpServer(s.name, v)" />
                {{ s.displayName }}
                <span class="text-xs text-muted-foreground">（{{ s.tools.filter(t => t.status === 'active').length }} 个工具）</span>
              </label>
            </div>
            <p v-else class="text-xs text-muted-foreground">没有接入站外 MCP 服务（在「MCP 工具」里接入）。</p>
          </div>
          <div class="grid gap-3 sm:grid-cols-2">
            <label class="flex items-center justify-between gap-4 text-sm">启用 <Switch v-model="characterDraft.enabled" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">在敲敲列表展示 <Switch v-model="characterDraft.knockVisible" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">允许私聊 <Switch v-model="characterDraft.dmEnabled" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">允许 @ 评论回复 <Switch v-model="characterDraft.commentEnabled" /></label>
            <label class="flex items-center justify-between gap-4 text-sm">允许 QQ 对话 <Switch v-model="characterDraft.qqEnabled" /></label>
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

    <!-- 调用记录 -->
    <Dialog :open="!!trace" @update:open="(v) => { if (!v) trace = null }">
      <DialogScrollContent v-if="trace" class="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>调用记录 · {{ jobKindLabel[trace.job.kind] ?? trace.job.kind }} {{ formatTime(trace.job.updatedAt) }}</DialogTitle>
        </DialogHeader>
        <div v-for="item in trace.items" :key="item.attempt" class="space-y-3">
          <div class="flex flex-wrap items-center gap-2 text-sm">
            <Badge variant="outline">第 {{ item.attempt }} 次</Badge>
            <span class="font-mono text-xs">{{ item.modelKey }}</span>
            <span class="text-xs text-muted-foreground">{{ item.outcome }} · {{ formatTime(item.createdAt) }}</span>
          </div>
          <div class="space-y-2">
            <div v-for="(m, index) in traceMessages(item.request)" :key="index" class="rounded-md border p-2">
              <div class="mb-1 text-xs font-medium text-muted-foreground">{{ m.role }}</div>
              <pre class="text-xs whitespace-pre-wrap break-words">{{ m.text }}</pre>
            </div>
            <div class="rounded-md border border-primary/40 p-2">
              <div class="mb-1 text-xs font-medium text-muted-foreground">回复</div>
              <pre class="text-xs whitespace-pre-wrap break-words">{{ item.reply || '（空）' }}</pre>
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="trace = null">关闭</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
