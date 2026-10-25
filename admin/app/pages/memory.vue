<script setup lang="ts">
import { Search, Trash2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber, formatTime } from '~/lib/format'

definePageMeta({ admin: true })

type Memory = Schemas['AiMemoryView']

const { run, failure } = useNotify()
const confirm = useConfirm()
const loading = ref(false)
const saving = ref(false)

const settings = ref<Schemas['AdminMemorySettings'] | null>(null)
const models = ref<Schemas['AdminModel'][]>([])
const providers = ref<string[]>([])
const characters = ref<Schemas['AdminCharacter'][]>([])

/** Select 不能用空值，用它表示「没选」。 */
const NONE = 'none'
const form = ref({
  enabled: true,
  modelKey: NONE,
  extractDelaySecs: 180,
  bridgeHours: 48,
  embeddingProvider: NONE,
  embeddingModel: '',
})

function fillForm(s: Schemas['AdminMemorySettings']) {
  form.value = {
    enabled: s.enabled,
    modelKey: s.modelKey ?? NONE,
    extractDelaySecs: s.extractDelaySecs,
    bridgeHours: s.bridgeHours,
    embeddingProvider: s.embeddingProvider ?? NONE,
    embeddingModel: s.embeddingModel ?? '',
  }
}

async function load() {
  loading.value = true
  try {
    const [s, m, a, c] = await Promise.all([
      unwrap(api.GET('/v1/admin/ai/memory')),
      unwrap(api.GET('/v1/admin/ai/models')),
      unwrap(api.GET('/v1/admin/ai/settings')),
      unwrap(api.GET('/v1/admin/ai/characters')),
    ])
    settings.value = s
    models.value = m
    providers.value = a.providers
    characters.value = c
    fillForm(s)
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

async function save() {
  const f = form.value
  if (!f.enabled && settings.value?.enabled) {
    const ok = await confirm({
      title: '关闭 AI 记忆？',
      description: 'AI 将不再读取、记下或整理任何记忆，也不提供记住 / 忘掉 / 翻聊天记录的工具；已有记忆保留，重新打开后恢复。',
      confirmLabel: '关闭',
      destructive: true,
    })
    if (!ok) return
  }
  const embedding = f.embeddingProvider !== NONE
  saving.value = true
  try {
    const saved = await run(
      () => unwrap(api.PUT('/v1/admin/ai/memory', {
        body: {
          enabled: f.enabled,
          modelKey: f.modelKey === NONE ? null : f.modelKey,
          extractDelaySecs: f.extractDelaySecs,
          bridgeHours: f.bridgeHours,
          embeddingProvider: embedding ? f.embeddingProvider : null,
          embeddingModel: embedding ? f.embeddingModel : null,
        },
      })),
      '已保存',
    )
    if (saved) {
      settings.value = saved
      fillForm(saved)
    }
  }
  finally {
    saving.value = false
  }
}

// ---------- 按用户查看 ----------

const query = ref('')
const found = ref<Schemas['AdminUserMemories'] | null>(null)
const searching = ref(false)
const clearCharacter = ref(NONE)

const kindLabel: Record<Memory['kind'], string> = { profile: '档案', relation: '关系', event: '事件' }
const originLabel: Record<Memory['origin'], string> = { extracted: '自动提炼', told: '对话里让记下', manual: '人工' }

function characterName(memory: Memory) {
  if (!memory.character) return memory.aiUserId ? `已删除的角色（${memory.aiUserId}）` : '所有角色'
  return characters.value.find(c => c.slug === memory.character)?.displayName ?? memory.character
}

async function lookup() {
  const user = query.value.trim()
  if (!user) return
  searching.value = true
  try {
    found.value = await unwrap(api.GET('/v1/admin/ai/memories', { params: { query: { user } } }))
    clearCharacter.value = NONE
  }
  catch (error) {
    found.value = null
    failure(error)
  }
  finally {
    searching.value = false
  }
}

async function remove(memory: Memory) {
  const ok = await confirm({
    title: '删除这条记忆？',
    description: memory.content,
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(
    () => unwrap(api.DELETE('/v1/admin/ai/memories/{id}', { params: { path: { id: memory.id } } })),
    '已删除',
  )
  if (done !== undefined && found.value) {
    found.value = { ...found.value, items: found.value.items.filter(m => m.id !== memory.id) }
  }
}

async function clearAll() {
  if (!found.value) return
  const character = clearCharacter.value === NONE ? null : clearCharacter.value
  const name = character ? (characters.value.find(c => c.slug === character)?.displayName ?? character) : null
  const ok = await confirm({
    title: name ? `清空 ${found.value.username} 与「${name}」的记忆？` : `清空 ${found.value.username} 的全部记忆？`,
    description: name
      ? '删除这个用户与该角色的关系记忆和事件记忆，档案保留。不可恢复。'
      : '删除这个用户的全部记忆（档案、关系、事件）。不可恢复。',
    confirmLabel: '清空',
    destructive: true,
  })
  if (!ok) return
  const done = await run(
    () => unwrap(api.POST('/v1/admin/ai/memories/clear', { body: { userId: found.value!.userId, character } })),
    '已清空',
  )
  if (done !== undefined) await lookup()
}
</script>

<template>
  <div>
    <AdminPageHeader title="AI 记忆" description="全站设置在本实例立即生效，其他实例最长 60 秒后生效。用户端没有任何记忆设置。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div class="grid gap-4 lg:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>设置</CardTitle>
          <CardDescription>
            记忆包括会话摘要、跨会话衔接、长期记忆（档案、关系、事件）、待跟进、每晚整理与遗忘。
            摘要、提炼、整理的模型调用不计入任何额度。
          </CardDescription>
        </CardHeader>
        <CardContent class="space-y-4">
          <Skeleton v-if="!settings" class="h-40 w-full" />
          <template v-else>
            <label class="flex items-center justify-between gap-4 text-sm">
              <span class="font-medium">启用 AI 记忆</span>
              <Switch v-model="form.enabled" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">摘要、提炼、整理用的模型</span>
              <Select v-model="form.modelKey">
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem :value="NONE">跟随全站默认模型</SelectItem>
                  <SelectItem v-for="m in models" :key="m.key" :value="m.key">
                    {{ m.displayName }}（{{ m.key }}）
                  </SelectItem>
                </SelectContent>
              </Select>
              <span class="text-xs text-muted-foreground">可选择未启用的模型。建议选便宜、擅长输出 JSON 的模型。</span>
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">对话停下来多久后提炼（秒）</span>
              <Input v-model.number="form.extractDelaySecs" type="number" min="0" max="86400" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">新会话带上其他会话多久以内的最近几轮（小时，0 关闭）</span>
              <Input v-model.number="form.bridgeHours" type="number" min="0" max="720" />
            </label>
            <div class="grid gap-2">
              <span class="text-sm leading-none font-medium">事件记忆的语义向量</span>
              <div class="grid gap-2 sm:grid-cols-2">
                <Select v-model="form.embeddingProvider">
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem :value="NONE">不用（只按关键词检索）</SelectItem>
                    <SelectItem v-for="p in providers" :key="p" :value="p">{{ p }}</SelectItem>
                  </SelectContent>
                </Select>
                <Input
                  v-model="form.embeddingModel"
                  :disabled="form.embeddingProvider === NONE"
                  placeholder="向量模型名，如 BAAI/bge-m3"
                />
              </div>
              <span class="text-xs text-muted-foreground">提供方需支持 OpenAI 兼容的 /embeddings 接口。</span>
            </div>
            <div class="flex items-center justify-between gap-4">
              <span class="text-xs text-muted-foreground">
                {{ settings.updatedAt ? `最近修改于 ${formatTime(settings.updatedAt)}` : '从未修改（使用默认值）' }}
              </span>
              <Button :disabled="saving" @click="save">保存</Button>
            </div>
          </template>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>统计</CardTitle>
          <CardDescription>全站当前保存的记忆。</CardDescription>
        </CardHeader>
        <CardContent>
          <Skeleton v-if="!settings" class="h-24 w-full" />
          <dl v-else class="grid grid-cols-2 gap-4 text-sm sm:grid-cols-3">
            <div>
              <dt class="text-muted-foreground">有记忆的用户</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.users) }}</dd>
            </div>
            <div>
              <dt class="text-muted-foreground">档案</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.profile) }}</dd>
            </div>
            <div>
              <dt class="text-muted-foreground">关系</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.relation) }}</dd>
            </div>
            <div>
              <dt class="text-muted-foreground">事件</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.event) }}</dd>
            </div>
            <div>
              <dt class="text-muted-foreground">待跟进</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.pendingFollowUps) }}</dd>
            </div>
            <div>
              <dt class="text-muted-foreground">已算向量</dt>
              <dd class="text-xl font-semibold tabular-nums">{{ formatNumber(settings.stats.withEmbedding) }}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>
    </div>

    <Card class="mt-4">
      <CardHeader>
        <CardTitle>按用户查看</CardTitle>
        <CardDescription>处理用户查看或删除记忆的请求。删除与清空都会记入审计日志。</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <form class="flex max-w-md gap-2" @submit.prevent="lookup">
          <Input v-model="query" placeholder="用户 id 或用户名" />
          <Button type="submit" :disabled="searching || !query.trim()">
            <Search class="size-4" />
            查找
          </Button>
        </form>

        <template v-if="found">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="text-sm">
              <span class="font-medium">{{ found.username }}</span>
              <span class="text-muted-foreground">（{{ found.userId }}）共 {{ found.items.length }} 条</span>
            </div>
            <div v-if="found.items.length" class="flex items-center gap-2">
              <Select v-model="clearCharacter">
                <SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem :value="NONE">全部记忆</SelectItem>
                  <SelectItem v-for="c in characters" :key="c.slug" :value="c.slug">与「{{ c.displayName }}」的</SelectItem>
                </SelectContent>
              </Select>
              <Button variant="destructive" @click="clearAll">清空</Button>
            </div>
          </div>

          <p v-if="!found.items.length" class="text-sm text-muted-foreground">这个用户没有记忆。</p>
          <Table v-else>
            <TableHeader>
              <TableRow>
                <TableHead class="w-16">种类</TableHead>
                <TableHead class="w-32">角色</TableHead>
                <TableHead>内容</TableHead>
                <TableHead class="w-36">来源</TableHead>
                <TableHead class="w-40">更新时间</TableHead>
                <TableHead class="w-12" />
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="m in found.items" :key="m.id">
                <TableCell><Badge variant="outline">{{ kindLabel[m.kind] }}</Badge></TableCell>
                <TableCell class="text-sm">{{ characterName(m) }}</TableCell>
                <TableCell class="text-sm">
                  <div>{{ m.content }}</div>
                  <div class="mt-1 flex flex-wrap gap-1">
                    <Badge v-if="m.pinned" variant="secondary">置顶</Badge>
                    <Badge v-if="m.sensitivity === 'public'" variant="secondary">公开</Badge>
                    <Badge v-if="m.importance !== 3" variant="secondary">重要度 {{ m.importance }}</Badge>
                    <Badge v-if="m.happenedOn" variant="secondary">{{ m.happenedOn }}</Badge>
                    <Badge v-if="m.followUpOn" variant="secondary">
                      {{ m.followedUpAt ? '已问起' : '待跟进' }} {{ m.followUpOn }}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell class="text-sm text-muted-foreground">{{ originLabel[m.origin] }}</TableCell>
                <TableCell class="text-sm text-muted-foreground">{{ formatTime(m.updatedAt) }}</TableCell>
                <TableCell>
                  <Button variant="ghost" size="icon" aria-label="删除" @click="remove(m)">
                    <Trash2 class="size-4" />
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </template>
      </CardContent>
    </Card>
  </div>
</template>
