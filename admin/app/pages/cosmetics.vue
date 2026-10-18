<script setup lang="ts">
import { Plus } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Dialog, DialogFooter, DialogHeader, DialogScrollContent, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { cosmeticKindLabel } from '~/lib/format'

type Avatar = Schemas['AdminAvatar']
type BusinessCard = Schemas['AdminCard']
type Kind = Schemas['CosmeticKind']

const { isAdmin } = useSession()
const { run, failure } = useNotify()
const confirm = useConfirm()

const tab = useQueryParam<'avatars' | 'cards'>('tab', 'avatars')
const avatars = ref<Avatar[] | null>(null)
const cards = ref<BusinessCard[] | null>(null)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    const [a, c] = await Promise.all([
      unwrap(api.GET('/v1/admin/cosmetics/avatars')),
      unwrap(api.GET('/v1/admin/cosmetics/cards')),
    ])
    avatars.value = a
    cards.value = c
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

// ---------- 编辑（头像与名片共用一个表单） ----------
interface Draft {
  type: 'avatar' | 'card'
  id: string | null
  name: string
  kind: Kind
  description: string
  story: string
  image: Schemas['MediaView'] | null
}

const draft = ref<Draft | null>(null)
const saving = ref(false)

function edit(type: Draft['type'], item?: Avatar | BusinessCard) {
  const card = item && 'story' in item ? item : undefined
  draft.value = {
    type,
    id: item?.id ?? null,
    name: item?.name ?? '',
    kind: item?.kind ?? 'character',
    description: card?.description ?? '',
    story: card?.story ?? '',
    image: item?.image ?? null,
  }
}

async function save() {
  const d = draft.value
  if (!d?.image) return
  saving.value = true
  let saved: unknown
  if (d.type === 'avatar') {
    const body = { name: d.name, kind: d.kind, imageId: d.image.id }
    saved = d.id
      ? await run(() => unwrap(api.PUT('/v1/admin/cosmetics/avatars/{id}', { params: { path: { id: d.id as string } }, body })), '已保存')
      : await run(() => unwrap(api.POST('/v1/admin/cosmetics/avatars', { body })), '已上架')
  }
  else {
    const body = { name: d.name, kind: d.kind, description: d.description, story: d.story, imageId: d.image.id }
    saved = d.id
      ? await run(() => unwrap(api.PUT('/v1/admin/cosmetics/cards/{id}', { params: { path: { id: d.id as string } }, body })), '已保存')
      : await run(() => unwrap(api.POST('/v1/admin/cosmetics/cards', { body })), '已上架')
  }
  saving.value = false
  if (saved) {
    draft.value = null
    load()
  }
}

async function remove(type: Draft['type'], item: Avatar | BusinessCard) {
  const ok = await confirm({
    title: `下架「${item.name}」？`,
    description: '正在装备它的用户会被自动卸下；此操作不可恢复。',
    confirmLabel: '下架',
    destructive: true,
  })
  if (!ok) return
  const path = { params: { path: { id: item.id } } }
  const call = type === 'avatar'
    ? api.DELETE('/v1/admin/cosmetics/avatars/{id}', path)
    : api.DELETE('/v1/admin/cosmetics/cards/{id}', path)
  const done = await run(() => unwrap(call).then(() => true), '已下架')
  if (done) load()
}
</script>

<template>
  <div>
    <AdminPageHeader title="装扮" description="系统头像与论坛名片的目录；用户在个人设置里装备。图片需要先上传。">
      <AdminRefreshButton :loading="loading" @click="load" />
      <Button v-if="isAdmin" size="sm" @click="edit(tab === 'avatars' ? 'avatar' : 'card')">
        <Plus class="size-4" />{{ tab === 'avatars' ? '新建头像' : '新建名片' }}
      </Button>
    </AdminPageHeader>

    <Tabs v-model="tab">
      <TabsList class="mb-4">
        <TabsTrigger value="avatars">系统头像<template v-if="avatars"> ({{ avatars.length }})</template></TabsTrigger>
        <TabsTrigger value="cards">论坛名片<template v-if="cards"> ({{ cards.length }})</template></TabsTrigger>
      </TabsList>

      <TabsContent value="avatars">
        <div class="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
          <Card v-for="avatar in avatars ?? []" :key="avatar.id" class="gap-0 overflow-hidden py-0">
            <img v-if="avatar.image" :src="avatar.image.thumbUrl" :alt="avatar.name" class="aspect-square w-full object-cover">
            <div v-else class="flex aspect-square items-center justify-center bg-muted text-xs text-muted-foreground">图片缺失</div>
            <CardContent class="space-y-2 p-3">
              <div class="flex items-center justify-between gap-2">
                <span class="truncate text-sm font-medium">{{ avatar.name }}</span>
                <Badge variant="outline">{{ cosmeticKindLabel[avatar.kind] }}</Badge>
              </div>
              <div v-if="isAdmin" class="flex gap-1">
                <Button size="sm" variant="outline" class="flex-1" @click="edit('avatar', avatar)">编辑</Button>
                <Button size="sm" variant="ghost" class="text-destructive" @click="remove('avatar', avatar)">下架</Button>
              </div>
            </CardContent>
          </Card>
        </div>
        <p v-if="avatars && avatars.length === 0" class="py-10 text-center text-sm text-muted-foreground">还没有系统头像</p>
      </TabsContent>

      <TabsContent value="cards">
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <Card v-for="card in cards ?? []" :key="card.id" class="gap-0 overflow-hidden py-0">
            <img v-if="card.image" :src="card.image.thumbUrl" :alt="card.name" class="aspect-video w-full object-cover">
            <div v-else class="flex aspect-video items-center justify-center bg-muted text-xs text-muted-foreground">图片缺失</div>
            <CardContent class="space-y-2 p-4">
              <div class="flex items-center justify-between gap-2">
                <span class="truncate font-medium">{{ card.name }}</span>
                <Badge variant="outline">{{ cosmeticKindLabel[card.kind] }}</Badge>
              </div>
              <p class="line-clamp-2 text-sm text-muted-foreground">{{ card.description || '（没有简介）' }}</p>
              <div v-if="isAdmin" class="flex gap-1">
                <Button size="sm" variant="outline" class="flex-1" @click="edit('card', card)">编辑</Button>
                <Button size="sm" variant="ghost" class="text-destructive" @click="remove('card', card)">下架</Button>
              </div>
            </CardContent>
          </Card>
        </div>
        <p v-if="cards && cards.length === 0" class="py-10 text-center text-sm text-muted-foreground">还没有论坛名片</p>
      </TabsContent>
    </Tabs>

    <Dialog :open="!!draft" @update:open="(v) => { if (!v) draft = null }">
      <DialogScrollContent v-if="draft" class="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{{ draft.id ? '编辑' : '新建' }}{{ draft.type === 'avatar' ? '系统头像' : '论坛名片' }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <AdminImagePicker v-model="draft.image" />
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">名称</span>
              <Input v-model="draft.name" maxlength="50" />
            </label>
            <div class="space-y-2">
              <Label>分类</Label>
              <Select v-model="draft.kind">
                <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="(label, key) in cosmeticKindLabel" :key="key" :value="key">{{ label }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <template v-if="draft.type === 'card'">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">简介</span>
              <Textarea v-model="draft.description" rows="2" maxlength="500" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">背景故事（Markdown）</span>
              <Textarea v-model="draft.story" rows="8" maxlength="20000" class="font-mono text-xs" />
            </label>
          </template>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="draft = null">取消</Button>
          <Button :disabled="saving || !draft.image || !draft.name.trim()" @click="save">保存</Button>
        </DialogFooter>
      </DialogScrollContent>
    </Dialog>
  </div>
</template>
