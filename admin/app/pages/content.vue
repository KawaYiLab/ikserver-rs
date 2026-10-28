<script setup lang="ts">
import { Ban, Pencil, Plus, RotateCcw, Trash2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatNumber, formatTime } from '~/lib/format'

type Category = Schemas['AdminCategory']
type Tag = Schemas['AdminTag']

const { isAdmin } = useSession()
const { run, failure } = useNotify()
const confirm = useConfirm()
const tab = useQueryParam<string>('tab', 'categories')

// ---------- 分区 ----------
const categories = ref<Category[] | null>(null)
const loadingCategories = ref(false)

async function loadCategories() {
  loadingCategories.value = true
  try {
    categories.value = await unwrap(api.GET('/v1/admin/categories'))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loadingCategories.value = false
  }
}

interface CategoryDraft extends Required<Schemas['CategoryInput']> {
  id: string | null
}

const categoryDraft = ref<CategoryDraft | null>(null)
const saving = ref(false)

function editCategory(category?: Category) {
  categoryDraft.value = category
    ? {
        id: category.id,
        name: category.name,
        slug: category.slug,
        description: category.description,
        sortOrder: category.sortOrder,
        isActive: category.isActive,
        adminOnly: category.adminOnly,
        allowCommentAccept: category.allowCommentAccept,
      }
    : {
        id: null,
        name: '',
        slug: '',
        description: '',
        sortOrder: (categories.value?.length ?? 0) * 10,
        isActive: true,
        adminOnly: false,
        allowCommentAccept: false,
      }
}

async function saveCategory() {
  const draft = categoryDraft.value
  if (!draft) return
  const { id, ...body } = draft
  saving.value = true
  const saved = id
    ? await run(() => unwrap(api.PUT('/v1/admin/categories/{id}', { params: { path: { id } }, body })), '分区已保存')
    : await run(() => unwrap(api.POST('/v1/admin/categories', { body })), '分区已创建')
  saving.value = false
  if (saved) {
    categoryDraft.value = null
    loadCategories()
  }
}

async function removeCategory(category: Category) {
  const ok = await confirm({
    title: `删除分区「${category.name}」？`,
    description: '只有没有帖子和草稿的分区可以删除；不想再用的分区可以改为停用。',
    confirmLabel: '删除',
    destructive: true,
  })
  if (!ok) return
  const done = await run(() => unwrap(api.DELETE('/v1/admin/categories/{id}', { params: { path: { id: category.id } } })).then(() => true), '已删除')
  if (done) loadCategories()
}

// ---------- 标签 ----------
const tagList = useListQuery({
  filters: { tagQ: '', tagBlocked: 'all' },
  text: ['tagQ'],
  pageKey: 'tagPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/tags', {
    params: {
      query: {
        q: optional(f.tagQ),
        blocked: optionalBool(f.tagBlocked),
        page,
        limit: 50,
      },
    },
  })),
})
const tags = tagList.result
const tagFilters = tagList.filters
const loadTags = tagList.refresh

function categoryActions(category: Category): RowAction[] {
  return [
    { label: '编辑', icon: Pencil, onSelect: () => editCategory(category) },
    {
      label: '删除',
      icon: Trash2,
      destructive: true,
      disabledReason: category.postCount > 0 && '还有帖子，只能停用',
      onSelect: () => removeCategory(category),
    },
  ]
}

function tagActions(tag: Tag): RowAction[] {
  return [
    { label: '改名', icon: Pencil, onSelect: () => startRename(tag) },
    tag.isBlocked
      ? { label: '恢复', icon: RotateCcw, onSelect: () => setBlocked(tag, false) }
      : { label: '屏蔽', icon: Ban, destructive: true, onSelect: () => setBlocked(tag, true) },
  ]
}

async function setBlocked(tag: Tag, isBlocked: boolean) {
  const updated = await run(() => unwrap(api.PATCH('/v1/admin/tags/{id}', {
    params: { path: { id: tag.id } },
    body: { isBlocked },
  })), isBlocked ? `已屏蔽 #${tag.name}` : `已恢复 #${tag.name}`)
  if (updated) loadTags()
}

const renaming = ref<Tag | null>(null)
const newName = ref('')

function startRename(tag: Tag) {
  renaming.value = tag
  newName.value = tag.name
}

async function rename() {
  const tag = renaming.value
  if (!tag) return
  const updated = await run(() => unwrap(api.PATCH('/v1/admin/tags/{id}', {
    params: { path: { id: tag.id } },
    body: { name: newName.value },
  })), '已改名')
  if (updated) {
    renaming.value = null
    loadTags()
  }
}

function refresh() {
  if (tab.value === 'categories') loadCategories()
  else loadTags()
}

onMounted(loadCategories)
</script>

<template>
  <div>
    <AdminPageHeader title="分区与标签">
      <AdminRefreshButton :loading="loadingCategories || tagList.loading.value" @click="refresh" />
      <Button v-if="isAdmin && tab === 'categories'" size="sm" @click="editCategory()">
        <Plus class="size-4" />新建分区
      </Button>
    </AdminPageHeader>

    <Tabs v-model="tab">
      <TabsList class="mb-4">
        <TabsTrigger value="categories">分区</TabsTrigger>
        <TabsTrigger value="tags">标签</TabsTrigger>
      </TabsList>

      <TabsContent value="categories">
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead class="w-16 text-right">排序</TableHead>
                <TableHead>名称</TableHead>
                <TableHead class="hidden md:table-cell">slug</TableHead>
                <TableHead class="text-right">帖子</TableHead>
                <TableHead>属性</TableHead>
                <TableHead v-if="isAdmin" class="w-10"><span class="sr-only">操作</span></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="category in categories ?? []" :key="category.id">
                <TableCell class="text-right tabular-nums">{{ category.sortOrder }}</TableCell>
                <TableCell>
                  <div class="font-medium">{{ category.name }}</div>
                  <div v-if="category.description" class="line-clamp-1 max-w-xs text-xs text-muted-foreground">{{ category.description }}</div>
                </TableCell>
                <TableCell class="hidden md:table-cell font-mono text-xs">{{ category.slug }}</TableCell>
                <TableCell class="text-right tabular-nums">{{ formatNumber(category.postCount) }}</TableCell>
                <TableCell>
                  <div class="flex flex-wrap gap-1">
                    <Badge :variant="category.isActive ? 'default' : 'outline'">{{ category.isActive ? '启用' : '停用' }}</Badge>
                    <Badge v-if="category.adminOnly" variant="secondary">仅管理员发帖</Badge>
                    <Badge v-if="category.allowCommentAccept" variant="secondary">可采纳评论</Badge>
                  </div>
                </TableCell>
                <TableCell v-if="isAdmin" class="text-right"><AdminRowActions :actions="categoryActions(category)" /></TableCell>
              </TableRow>
              <AdminListState
                :colspan="isAdmin ? 6 : 5" :loading="loadingCategories" :empty="!categories?.length"
                :failed="!categories && !loadingCategories" empty-text="还没有分区"
                @retry="loadCategories"
              />
            </TableBody>
          </Table>
        </div>
      </TabsContent>

      <TabsContent value="tags">
        <div class="mb-4 grid gap-2 sm:grid-cols-3">
          <Input v-model="tagFilters.tagQ" placeholder="搜索标签" class="sm:col-span-2" />
          <Select v-model="tagFilters.tagBlocked">
            <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部标签</SelectItem>
              <SelectItem value="no">正常</SelectItem>
              <SelectItem value="yes">已屏蔽</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>标签</TableHead>
                <TableHead class="hidden md:table-cell">slug</TableHead>
                <TableHead class="text-right">帖子</TableHead>
                <TableHead class="hidden md:table-cell">创建时间</TableHead>
                <TableHead>状态</TableHead>
                <TableHead v-if="isAdmin" class="w-10"><span class="sr-only">操作</span></TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="tag in tags?.items ?? []" :key="tag.id">
                <TableCell class="font-medium">#{{ tag.name }}</TableCell>
                <TableCell class="hidden md:table-cell font-mono text-xs">{{ tag.slug }}</TableCell>
                <TableCell class="text-right tabular-nums">{{ formatNumber(tag.postCount) }}</TableCell>
                <TableCell class="hidden md:table-cell">{{ formatTime(tag.createdAt) }}</TableCell>
                <TableCell>
                  <Badge :variant="tag.isBlocked ? 'destructive' : 'outline'">{{ tag.isBlocked ? '已屏蔽' : '正常' }}</Badge>
                </TableCell>
                <TableCell v-if="isAdmin" class="text-right"><AdminRowActions :actions="tagActions(tag)" /></TableCell>
              </TableRow>
              <AdminListState
                :colspan="isAdmin ? 6 : 5" :loading="tagList.loading.value" :failed="tagList.failed.value" :filtered="tagList.active.value"
                :empty="!tags?.items.length" empty-text="还没有标签"
                @retry="loadTags" @reset="tagList.reset"
              />
            </TableBody>
          </Table>
        </div>
        <AdminPager v-if="tags" :page="tags.page" :limit="tags.limit" :total="tags.total" unit="个标签" @change="tagList.setPage" />
      </TabsContent>
    </Tabs>

    <Dialog :open="!!categoryDraft" @update:open="(v) => { if (!v) categoryDraft = null }">
      <DialogContent v-if="categoryDraft">
        <DialogHeader>
          <DialogTitle>{{ categoryDraft.id ? '编辑分区' : '新建分区' }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-4">
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">名称</span>
              <Input v-model="categoryDraft.name" maxlength="30" />
            </label>
            <label class="grid gap-2">
              <span class="text-sm leading-none font-medium">slug（小写字母、数字、连字符）</span>
              <Input v-model="categoryDraft.slug" maxlength="48" class="font-mono" />
            </label>
          </div>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">简介</span>
            <Textarea v-model="categoryDraft.description" rows="2" />
          </label>
          <label class="grid gap-2">
            <span class="text-sm leading-none font-medium">排序（小的在前）</span>
            <Input v-model.number="categoryDraft.sortOrder" type="number" class="w-32" />
          </label>
          <div class="space-y-3">
            <label class="flex items-center justify-between gap-4 text-sm">
              启用
              <Switch v-model="categoryDraft.isActive" />
            </label>
            <label class="flex items-center justify-between gap-4 text-sm">
              仅管理员可发帖
              <Switch v-model="categoryDraft.adminOnly" />
            </label>
            <label class="flex items-center justify-between gap-4 text-sm">
              允许楼主采纳评论
              <Switch v-model="categoryDraft.allowCommentAccept" />
            </label>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="categoryDraft = null">取消</Button>
          <Button :disabled="saving" @click="saveCategory">保存</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <Dialog :open="!!renaming" @update:open="(v) => { if (!v) renaming = null }">
      <DialogContent v-if="renaming">
        <DialogHeader>
          <DialogTitle>重命名 #{{ renaming.name }}</DialogTitle>
        </DialogHeader>
        <div class="space-y-2">
          <Input v-model="newName" @keydown.enter="rename" />
          <p class="text-xs text-muted-foreground">按发帖时的规则归一化，slug 随名称重新生成；已有帖子上的标签一并改名。</p>
        </div>
        <DialogFooter>
          <Button variant="outline" @click="renaming = null">取消</Button>
          <Button :disabled="!newName.trim()" @click="rename">保存</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
