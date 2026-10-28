<script setup lang="ts">
import { Plus } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Dialog, DialogFooter, DialogHeader, DialogScrollContent, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'

type Group = Schemas['AdminEmoteGroup']
type Emote = Schemas['AdminEmote']

/** Select 不接受空字符串的值，用它表示「未分组」。 */
const NO_GROUP = '__none__'

const { isAdmin } = useSession()
const { run, failure } = useNotify()
const confirm = useConfirm()

const groups = ref<Group[] | null>(null)
const emotes = ref<Emote[] | null>(null)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    const data = await unwrap(api.GET('/v1/admin/emotes'))
    groups.value = data.groups
    emotes.value = data.emotes
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

/** 按分组排好的表情；未分组的放最后。 */
const sections = computed(() => {
  const list = emotes.value ?? []
  const result: { key: string, group: Group | null, items: Emote[] }[] = (groups.value ?? []).map(group => ({
    key: group.id,
    group,
    items: list.filter(e => e.groupId === group.id),
  }))
  const ungrouped = list.filter(e => !e.groupId || !groups.value?.some(g => g.id === e.groupId))
  if (ungrouped.length) result.push({ key: NO_GROUP, group: null, items: ungrouped })
  return result
})

// ---------- 表情 ----------
interface EmoteDraft {
  id: string | null
  code: string
  name: string
  groupId: string
  sortOrder: number
  active: boolean
  image: Schemas['MediaView'] | null
}

const emoteDraft = ref<EmoteDraft | null>(null)
const saving = ref(false)

function editEmote(item?: Emote, groupId?: string) {
  emoteDraft.value = {
    id: item?.id ?? null,
    code: item?.code ?? 'ik-',
    name: item?.name ?? '',
    groupId: item?.groupId ?? groupId ?? NO_GROUP,
    sortOrder: item?.sortOrder ?? 0,
    active: item?.active ?? true,
    image: item?.image ?? null,
  }
}

async function saveEmote() {
  const d = emoteDraft.value
  if (!d?.image) return
  saving.value = true
  const body = {
    name: d.name,
    groupId: d.groupId === NO_GROUP ? null : d.groupId,
    imageId: d.image.id,
    sortOrder: Number(d.sortOrder) || 0,
    active: d.active,
  }
  const saved = d.id
    ? await run(() => unwrap(api.PUT('/v1/admin/emotes/{id}', { params: { path: { id: d.id as string } }, body })), '已保存')
    : await run(() => unwrap(api.POST('/v1/admin/emotes', { body: { ...body, code: d.code.trim() } })), '已添加')
  saving.value = false
  if (saved) {
    emoteDraft.value = null
    load()
  }
}

async function removeEmote(item: Emote) {
  const ok = await confirm({
    title: `删除表情「${item.name}」？`,
    description: `删除后旧评论里的 :${item.code}: 会显示成文字。只想隐藏请改为下架。`,
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(
    () => unwrap(api.DELETE('/v1/admin/emotes/{id}', { params: { path: { id: item.id } } })).then(() => true),
    '已删除',
  )
  if (done) load()
}

// ---------- 分组 ----------
interface GroupDraft {
  id: string | null
  name: string
  sortOrder: number
  active: boolean
  icon: Schemas['MediaView'] | null
}

const groupDraft = ref<GroupDraft | null>(null)

function editGroup(item?: Group) {
  groupDraft.value = {
    id: item?.id ?? null,
    name: item?.name ?? '',
    sortOrder: item?.sortOrder ?? 0,
    active: item?.active ?? true,
    icon: item?.icon ?? null,
  }
}

async function saveGroup() {
  const d = groupDraft.value
  if (!d) return
  saving.value = true
  const body = { name: d.name, sortOrder: Number(d.sortOrder) || 0, active: d.active, iconId: d.icon?.id ?? null }
  const saved = d.id
    ? await run(() => unwrap(api.PUT('/v1/admin/emotes/groups/{id}', { params: { path: { id: d.id as string } }, body })), '已保存')
    : await run(() => unwrap(api.POST('/v1/admin/emotes/groups', { body })), '已添加')
  saving.value = false
  if (saved) {
    groupDraft.value = null
    load()
  }
}

async function removeGroup(item: Group) {
  const ok = await confirm({
    title: `删除分组「${item.name}」？`,
    description: '组里的表情会保留，变成未分组。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(
    () => unwrap(api.DELETE('/v1/admin/emotes/groups/{id}', { params: { path: { id: item.id } } })).then(() => true),
    '已删除',
  )
  if (done) load()
}
</script>

<template>
  <div>
    <AdminPageHeader title="表情包" description="在评论里用 :代码: 插入表情。">
      <AdminRefreshButton :loading="loading" @click="load" />
      <template v-if="isAdmin">
        <Button variant="outline" size="sm" @click="editGroup()">
          <Plus class="size-4" />新建分组
        </Button>
        <Button size="sm" @click="editEmote()">
          <Plus class="size-4" />添加表情
        </Button>
      </template>
    </AdminPageHeader>

    <div class="space-y-6">
      <section v-for="section in sections" :key="section.key" class="space-y-3">
        <div class="flex flex-wrap items-center gap-2">
          <img v-if="section.group?.icon" :src="section.group.icon.url" :alt="section.group.name" class="size-6 object-contain">
          <h2 class="font-medium">{{ section.group ? section.group.name : '未分组' }}</h2>
          <span class="text-sm text-muted-foreground">{{ section.items.length }} 个</span>
          <Badge v-if="section.group && !section.group.active" variant="secondary">已停用</Badge>
          <Badge v-if="section.group" variant="outline">排序 {{ section.group.sortOrder }}</Badge>
          <div v-if="isAdmin && section.group" class="ml-auto flex gap-1">
            <Button size="sm" variant="ghost" @click="editEmote(undefined, section.group.id)">添加表情</Button>
            <Button size="sm" variant="outline" @click="editGroup(section.group)">编辑分组</Button>
            <Button size="sm" variant="ghost" class="text-destructive" @click="removeGroup(section.group)">删除分组</Button>
          </div>
        </div>
        <div class="grid grid-cols-3 gap-3 sm:grid-cols-4 lg:grid-cols-8">
          <Card v-for="emote in section.items" :key="emote.id" class="gap-0 overflow-hidden py-0" :class="{ 'opacity-50': !emote.active }">
            <div class="flex aspect-square items-center justify-center bg-muted p-3">
              <img v-if="emote.image" :src="emote.image.url" :alt="emote.name" class="max-h-full max-w-full object-contain">
              <span v-else class="text-xs text-muted-foreground">图片缺失</span>
            </div>
            <CardContent class="space-y-1 p-2">
              <div class="truncate text-sm font-medium">{{ emote.name }}</div>
              <div class="truncate font-mono text-xs text-muted-foreground">:{{ emote.code }}:</div>
              <Badge v-if="!emote.active" variant="secondary">已下架</Badge>
              <div v-if="isAdmin" class="flex gap-1 pt-1">
                <Button size="sm" variant="outline" class="flex-1" @click="editEmote(emote)">编辑</Button>
                <Button size="sm" variant="ghost" class="text-destructive" @click="removeEmote(emote)">删除</Button>
              </div>
            </CardContent>
          </Card>
        </div>
        <p v-if="section.items.length === 0" class="text-sm text-muted-foreground">这个分组还没有表情</p>
      </section>
      <p v-if="emotes && emotes.length === 0 && groups && groups.length === 0" class="py-10 text-center text-sm text-muted-foreground">
        还没有表情
      </p>
    </div>

    <Dialog :open="!!emoteDraft" @update:open="(v) => { if (!v) emoteDraft = null }">
      <DialogScrollContent v-if="emoteDraft" class="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{{ emoteDraft.id ? '编辑表情' : '添加表情' }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <AdminImagePicker v-model="emoteDraft.image" />
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">代码</span>
              <Input v-model="emoteDraft.code" :disabled="!!emoteDraft.id" maxlength="35" class="font-mono" placeholder="ik-smile" />
              <span class="text-xs text-muted-foreground">ik- 开头，小写字母、数字、连字符；建好后不能改</span>
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">名称</span>
              <Input v-model="emoteDraft.name" maxlength="30" />
            </label>
            <div class="space-y-2">
              <Label>分组</Label>
              <Select v-model="emoteDraft.groupId">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem :value="NO_GROUP">未分组</SelectItem>
                  <SelectItem v-for="group in groups ?? []" :key="group.id" :value="group.id">{{ group.name }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">排序（小的在前）</span>
              <Input v-model.number="emoteDraft.sortOrder" type="number" />
            </label>
          </div>
          <div class="flex items-center gap-2">
            <Switch v-model="emoteDraft.active" />
            <span class="text-sm">在表情面板里显示</span>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="emoteDraft = null">取消</Button>
          <Button :disabled="saving || !emoteDraft.image || !emoteDraft.name.trim() || !emoteDraft.code.trim()" @click="saveEmote">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>

    <Dialog :open="!!groupDraft" @update:open="(v) => { if (!v) groupDraft = null }">
      <DialogScrollContent v-if="groupDraft" class="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{{ groupDraft.id ? '编辑分组' : '新建分组' }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="space-y-2">
            <Label>图标（可选）</Label>
            <AdminImagePicker v-model="groupDraft.icon" />
            <Button v-if="groupDraft.icon" size="sm" variant="ghost" @click="groupDraft.icon = null">清除图标</Button>
          </div>
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">名称</span>
              <Input v-model="groupDraft.name" maxlength="20" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">排序（小的在前）</span>
              <Input v-model.number="groupDraft.sortOrder" type="number" />
            </label>
          </div>
          <div class="flex items-center gap-2">
            <Switch v-model="groupDraft.active" />
            <span class="text-sm">启用</span>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="groupDraft = null">取消</Button>
          <Button :disabled="saving || !groupDraft.name.trim()" @click="saveGroup">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
