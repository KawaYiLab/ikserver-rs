<script setup lang="ts">
import { Pencil, Plus, Trash2, X } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogFooter, DialogHeader, DialogScrollContent, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatTime, kindLabel } from '~/lib/format'

type Question = Schemas['AdminQuestion']
type Kind = Schemas['QuestionKind']

const { isAdmin } = useSession()
const { run, success } = useNotify()
const confirm = useConfirm()
const badges = useBadges()

const stats = ref<Schemas['QuestionStats'] | null>(null)
const list = useListQuery({
  filters: { kind: 'all', enabled: 'all', q: '' },
  text: ['q'],
  fetch: async (f, page) => {
    const [questions, counts] = await Promise.all([
      unwrap(api.GET('/v1/admin/exam/questions', {
        params: {
          query: {
            type: optional(f.kind),
            enabled: optionalBool(f.enabled, 'on'),
            q: optional(f.q),
            page,
            limit: 20,
          },
        },
      })),
      unwrap(api.GET('/v1/admin/exam/questions/stats')),
    ])
    stats.value = counts
    return questions
  },
})
const { filters, result, loading, selection } = list
const select = useSelection(selection, () => result.value?.items.map(item => item.id) ?? [])

function load() {
  list.refresh()
  badges.refresh()
}

const short = computed(() => !!stats.value && stats.value.enabled < stats.value.questionCount)

function rowActions(question: Question): RowAction[] {
  return [
    { label: '编辑', icon: Pencil, onSelect: () => edit(question) },
    { label: '删除', icon: Trash2, destructive: true, onSelect: () => remove([question.id]) },
  ]
}

async function toggle(question: Question) {
  const updated = await run(() => unwrap(api.POST('/v1/admin/exam/questions/{id}/toggle', {
    params: { path: { id: question.id } },
  })))
  if (updated) load()
}

async function remove(ids: string[]) {
  const ok = await confirm({
    title: `删除 ${ids.length} 道题？`,
    description: '删除后不可恢复；进行中的考卷不受影响（考卷保存的是快照）。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.POST('/v1/admin/exam/questions/bulk-delete', { body: { ids } })))
  if (done) {
    success(`已删除 ${done.deletedCount} 道题`)
    load()
  }
}

// ---------- 编辑 ----------
interface Draft {
  id: string | null
  question: string
  kind: Kind
  options: { key: string, text: string }[]
  correct: string[]
  explanation: string
  tag: string
  weight: number
  enabled: boolean
}

const KEYS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
const draft = ref<Draft | null>(null)
const saving = ref(false)

function blank(): Draft {
  return {
    id: null,
    question: '',
    kind: 'single',
    options: [{ key: 'A', text: '' }, { key: 'B', text: '' }, { key: 'C', text: '' }, { key: 'D', text: '' }],
    correct: [],
    explanation: '',
    tag: '',
    weight: 1,
    enabled: true,
  }
}

function edit(question?: Question) {
  draft.value = question
    ? {
        id: question.id,
        question: question.question,
        kind: question.kind,
        options: question.options.map(o => ({ ...o })),
        correct: [...question.correctKeys],
        explanation: question.explanation ?? '',
        tag: question.tag ?? '',
        weight: question.weight,
        enabled: question.enabled,
      }
    : blank()
}

/** 改题型时套用对应的选项形状（判断题固定「正确 / 错误」）。 */
function changeKind(next: Kind) {
  const d = draft.value
  if (!d) return
  d.kind = next
  if (next === 'boolean') {
    d.options = [{ key: 'A', text: '正确' }, { key: 'B', text: '错误' }]
    d.correct = d.correct.filter(k => k === 'A' || k === 'B').slice(0, 1)
  }
  else if (next === 'single') {
    d.correct = d.correct.slice(0, 1)
  }
}

function relabel(d: Draft) {
  const mapping = new Map(d.options.map((o, i) => [o.key, KEYS[i] ?? o.key]))
  d.options = d.options.map((o, i) => ({ key: KEYS[i] ?? o.key, text: o.text }))
  d.correct = d.correct.map(k => mapping.get(k) ?? k)
}

function addOption() {
  const d = draft.value
  if (!d || d.options.length >= KEYS.length) return
  d.options.push({ key: KEYS[d.options.length] ?? '', text: '' })
}

function removeOption(index: number) {
  const d = draft.value
  if (!d) return
  const [gone] = d.options.splice(index, 1)
  d.correct = d.correct.filter(k => k !== gone?.key)
  relabel(d)
}

function setCorrect(key: string, on: boolean | 'indeterminate') {
  const d = draft.value
  if (!d) return
  if (d.kind !== 'multiple') {
    d.correct = on === true ? [key] : []
    return
  }
  d.correct = on === true ? [...new Set([...d.correct, key])].sort() : d.correct.filter(k => k !== key)
}

async function save() {
  const d = draft.value
  if (!d) return
  const body: Schemas['QuestionInput'] = {
    question: d.question,
    kind: d.kind,
    options: d.options.map(o => ({ key: o.key, text: o.text.trim() })),
    correctKeys: d.correct,
    explanation: d.explanation.trim() || null,
    tag: d.tag.trim() || null,
    weight: d.weight,
    enabled: d.enabled,
  }
  saving.value = true
  const saved = d.id
    ? await run(() => unwrap(api.PUT('/v1/admin/exam/questions/{id}', { params: { path: { id: d.id as string } }, body })), '已保存')
    : await run(() => unwrap(api.POST('/v1/admin/exam/questions', { body })), '已新建')
  saving.value = false
  if (saved) {
    draft.value = null
    load()
  }
}
</script>

<template>
  <div>
    <AdminPageHeader title="考试题库" description="入站考试从启用的题目里按权重抽题；考卷保存题目快照，改题不影响进行中的考试。">
      <AdminRefreshButton :loading="loading" @click="load" />
      <Button v-if="isAdmin" size="sm" @click="edit()">
        <Plus class="size-4" />新建题目
      </Button>
    </AdminPageHeader>

    <div v-if="stats" class="mb-6 grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
      <AdminStatCard title="题目总数" :value="stats.total" />
      <AdminStatCard
        title="已启用"
        :value="stats.enabled"
        :hint="short ? `少于每张考卷的 ${stats.questionCount} 题，考生将无法开考` : `每张考卷 ${stats.questionCount} 题`"
        :tone="short ? 'danger' : 'default'"
      />
      <AdminStatCard title="单选（启用）" :value="stats.single" />
      <AdminStatCard title="多选（启用）" :value="stats.multiple" />
      <AdminStatCard title="判断（启用）" :value="stats.boolean" />
    </div>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
      <Select v-model="filters.kind">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部题型</SelectItem>
          <SelectItem v-for="(label, key) in kindLabel" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.enabled">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem value="on">已启用</SelectItem>
          <SelectItem value="off">已停用</SelectItem>
        </SelectContent>
      </Select>
      <Input v-model="filters.q" placeholder="搜索题干或标签" class="lg:col-span-2" />
    </div>

    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead v-if="isAdmin" class="w-10">
              <Checkbox :model-value="select.all.value" aria-label="全选本页" @update:model-value="select.toggleAll" />
            </TableHead>
            <TableHead>题干</TableHead>
            <TableHead class="hidden md:table-cell">题型</TableHead>
            <TableHead class="hidden lg:table-cell">标签</TableHead>
            <TableHead class="hidden md:table-cell text-right">权重</TableHead>
            <TableHead>启用</TableHead>
            <TableHead class="hidden lg:table-cell">更新时间</TableHead>
            <TableHead v-if="isAdmin" class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="item in result?.items ?? []" :key="item.id">
            <TableCell v-if="isAdmin">
              <Checkbox :model-value="select.has(item.id)" aria-label="选择" @update:model-value="(v) => select.toggle(item.id, v)" />
            </TableCell>
            <TableCell class="max-w-md">
              <button class="line-clamp-2 text-left hover:underline" @click="edit(item)">{{ item.question }}</button>
              <div class="mt-1 text-xs text-muted-foreground">答案：{{ item.correctKeys.join('、') }}</div>
            </TableCell>
            <TableCell class="hidden md:table-cell">{{ kindLabel[item.kind] }}</TableCell>
            <TableCell class="hidden lg:table-cell">
              <Badge v-if="item.tag" variant="outline">{{ item.tag }}</Badge>
            </TableCell>
            <TableCell class="hidden md:table-cell text-right tabular-nums">{{ item.weight }}</TableCell>
            <TableCell>
              <Switch v-if="isAdmin" :model-value="item.enabled" aria-label="启用" @update:model-value="toggle(item)" />
              <Badge v-else :variant="item.enabled ? 'default' : 'outline'">{{ item.enabled ? '启用' : '停用' }}</Badge>
            </TableCell>
            <TableCell class="hidden lg:table-cell">{{ formatTime(item.updatedAt) }}</TableCell>
            <TableCell v-if="isAdmin" class="text-right"><AdminRowActions :actions="rowActions(item)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="isAdmin ? 8 : 6" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" empty-text="题库还是空的"
            @retry="load" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="题" @change="list.setPage" />

    <AdminBulkBar v-if="isAdmin" :count="selection.size" unit="题" @clear="select.clear">
      <Button size="sm" variant="destructive" @click="remove([...selection])">
        <Trash2 class="size-4" />批量删除
      </Button>
    </AdminBulkBar>

    <Dialog :open="!!draft" @update:open="(v) => { if (!v) draft = null }">
      <DialogScrollContent v-if="draft" class="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{{ draft.id ? (isAdmin ? '编辑题目' : '查看题目') : '新建题目' }}</DialogTitle>
        </DialogHeader>
        <fieldset :disabled="!isAdmin" class="space-y-4">
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">题干</span>
            <Textarea v-model="draft.question" rows="3" maxlength="2000" />
          </label>
          <div class="grid gap-4 sm:grid-cols-3">
            <div class="space-y-2">
              <Label>题型</Label>
              <Select :model-value="draft.kind" :disabled="!isAdmin" @update:model-value="(v) => changeKind(v as Kind)">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="(label, key) in kindLabel" :key="key" :value="key">{{ label }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">权重</span>
              <Input v-model.number="draft.weight" type="number" min="1" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">标签（选填）</span>
              <Input v-model="draft.tag" />
            </label>
          </div>
          <div class="space-y-2">
            <Label>选项（勾选正确答案{{ draft.kind === 'multiple' ? '，至少 2 项' : '' }}）</Label>
            <div v-for="(option, index) in draft.options" :key="index" class="flex items-center gap-2">
              <Checkbox
                :model-value="draft.correct.includes(option.key)"
                :disabled="!isAdmin"
                :aria-label="`${option.key} 是正确答案`"
                @update:model-value="(v) => setCorrect(option.key, v)"
              />
              <span class="w-5 font-mono text-sm">{{ option.key }}</span>
              <Input v-model="option.text" :readonly="draft.kind === 'boolean'" />
              <Button
                v-if="isAdmin && draft.kind !== 'boolean'"
                size="icon"
                variant="ghost"
                :disabled="draft.options.length <= 2"
                aria-label="删除选项"
                @click="removeOption(index)"
              >
                <X class="size-4" />
              </Button>
            </div>
            <Button v-if="isAdmin && draft.kind !== 'boolean'" size="sm" variant="outline" type="button" @click="addOption">
              <Plus class="size-4" />添加选项
            </Button>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">解析（选填）</span>
            <Textarea v-model="draft.explanation" rows="2" />
          </label>
          <div class="flex items-center gap-2">
            <Switch v-model="draft.enabled" :disabled="!isAdmin" />
            <Label>启用</Label>
          </div>
        </fieldset>
        <DialogFooter>
          <Button variant="outline" @click="draft = null">{{ isAdmin ? '取消' : '关闭' }}</Button>
          <Button v-if="isAdmin" :disabled="saving" @click="save">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
