<script setup lang="ts">
import { ArrowRightLeft, Eye, EyeOff, ExternalLink, MessageSquare, Pin, PinOff, Trash2, X } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatNumber, formatTime } from '~/lib/format'
import { hiddenReasonLabel, userPath } from '~/lib/links'

type Post = Schemas['AdminPost']

const { isAdmin } = useSession()
const siteLink = useSiteLink()
const { run } = useNotify()
const moderate = useModerate()

const categories = ref<Schemas['AdminCategory'][]>([])
onMounted(async () => {
  categories.value = await unwrap(api.GET('/v1/admin/categories')).catch(() => [])
})

const list = useListQuery({
  filters: { q: '', categoryId: 'all', state: 'all', flag: 'all', authorId: '' },
  text: ['q'],
  fetch: (f, page) => unwrap(api.GET('/v1/admin/posts', {
    params: {
      query: {
        q: optional(f.q),
        categoryId: optional(f.categoryId),
        authorId: optional(f.authorId),
        visibility: f.state === 'visible' ? 'visible' : f.state === 'all' ? undefined : 'hidden',
        hiddenReason: f.state === 'moderator' || f.state === 'auto_report' ? f.state : undefined,
        pinned: f.flag === 'pinned' ? true : undefined,
        anonymous: f.flag === 'anonymous' ? true : f.flag === 'named' ? false : undefined,
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading, selection } = list
const select = useSelection(selection, () => result.value?.items.map(item => item.id) ?? [])
const items = computed(() => result.value?.items ?? [])
const selectedPosts = computed(() => items.value.filter(post => selection.value.has(post.id)))

/** 按作者筛选时显示的作者名（从当前结果里取）。 */
const authorName = computed(() => {
  const id = list.applied.value.filters.authorId
  return items.value.find(post => post.author?.id === id)?.author?.username ?? id
})

async function act(posts: Post[], action: ModerateAction) {
  const label = posts.length === 1 ? `委托《${posts[0]?.title}》` : ` ${posts.length} 篇委托`
  const done = await moderate({ targetType: 'post', ids: posts.map(post => post.id), action, label })
  if (done) list.refresh()
}

async function setPinned(post: Post, pinned: boolean) {
  const path = { params: { path: { id: post.id } } }
  const done = await run(
    () => unwrap(pinned ? api.PUT('/v1/admin/posts/{id}/pin', path) : api.DELETE('/v1/admin/posts/{id}/pin', path)).then(() => true),
    pinned ? '已置顶' : '已取消置顶',
  )
  if (done) list.refresh()
}

// ---------- 移动分区 ----------
const moving = ref<Post | null>(null)
const moveTo = ref('')
const activeCategories = computed(() => categories.value.filter(c => c.isActive && (isAdmin.value || !c.adminOnly)))

function startMove(post: Post) {
  moving.value = post
  moveTo.value = post.category?.id ?? ''
}

async function move() {
  const post = moving.value
  if (!post || !moveTo.value) return
  const done = await run(() => unwrap(api.PUT('/v1/admin/posts/{id}/category', {
    params: { path: { id: post.id } },
    body: { categoryId: moveTo.value },
  })).then(() => true), '已移动')
  if (done) {
    moving.value = null
    list.refresh()
  }
}

function rowActions(post: Post): RowAction[] {
  const hidden = !!post.hiddenAt
  return [
    { label: '在主站打开', icon: ExternalLink, href: siteLink(`/post/${post.id}`) },
    { label: '查看评论', icon: MessageSquare, onSelect: () => navigateTo({ path: '/comments', query: { postId: post.id } }) },
    { label: '移动分区', icon: ArrowRightLeft, onSelect: () => startMove(post) },
    {
      label: post.pinnedAt ? '取消置顶' : '置顶',
      icon: post.pinnedAt ? PinOff : Pin,
      hidden: !isAdmin.value,
      disabledReason: !post.pinnedAt && hidden && '隐藏的帖子不能置顶',
      onSelect: () => setPinned(post, !post.pinnedAt),
    },
    hidden
      ? { label: '恢复可见', icon: Eye, onSelect: () => act([post], 'unhide') }
      : { label: '隐藏', icon: EyeOff, destructive: true, onSelect: () => act([post], 'hide') },
    { label: '删除', icon: Trash2, destructive: true, onSelect: () => act([post], 'delete') },
  ]
}
</script>

<template>
  <div>
    <AdminPageHeader title="帖子" description="不经举报直接隐藏、恢复或删除委托；有待处理举报的请到举报处理里结案。匿名委托不显示作者。">
      <AdminRefreshButton :loading="loading" @click="list.refresh" />
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-5">
      <Input v-model="filters.q" placeholder="标题 / 帖子 ID" class="lg:col-span-2" />
      <Select v-model="filters.categoryId">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部分区</SelectItem>
          <SelectItem v-for="c in categories" :key="c.id" :value="c.id">{{ c.name }}{{ c.isActive ? '' : '（停用）' }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.state">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem value="visible">公开</SelectItem>
          <SelectItem value="hidden">已隐藏</SelectItem>
          <SelectItem value="moderator">版主隐藏</SelectItem>
          <SelectItem value="auto_report">举报自动隐藏</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="filters.flag">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部委托</SelectItem>
          <SelectItem value="pinned">只看置顶</SelectItem>
          <SelectItem value="anonymous">只看匿名</SelectItem>
          <SelectItem value="named">排除匿名</SelectItem>
        </SelectContent>
      </Select>
    </div>
    <div v-if="list.applied.value.filters.authorId" class="mb-4 flex items-center gap-2 text-sm">
      <Badge variant="secondary" class="gap-1 py-1">
        作者：@{{ authorName }}
        <button type="button" aria-label="取消按作者筛选" class="rounded-full hover:bg-background/60" @click="filters.authorId = ''">
          <X class="size-3" />
        </button>
      </Badge>
      <span class="text-xs text-muted-foreground">按作者筛选时不包含匿名委托</span>
    </div>

    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-10">
              <Checkbox :model-value="select.all.value" aria-label="全选本页" @update:model-value="select.toggleAll" />
            </TableHead>
            <TableHead>委托</TableHead>
            <TableHead class="hidden md:table-cell">分区</TableHead>
            <TableHead>作者</TableHead>
            <TableHead class="hidden lg:table-cell text-right">浏览 / 赞 / 评论</TableHead>
            <TableHead class="hidden md:table-cell">发布时间</TableHead>
            <TableHead class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="post in items" :key="post.id" :data-state="select.has(post.id) ? 'selected' : undefined">
            <TableCell>
              <Checkbox :model-value="select.has(post.id)" aria-label="选择" @update:model-value="(v) => select.toggle(post.id, v)" />
            </TableCell>
            <TableCell class="max-w-md">
              <a :href="siteLink(`/post/${post.id}`)" target="_blank" rel="noopener" class="line-clamp-1 font-medium hover:underline" :class="{ 'text-muted-foreground line-through decoration-muted-foreground/40': post.hiddenAt }">
                {{ post.title }}
              </a>
              <div class="mt-1 flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
                <span class="font-mono">{{ post.id }}</span>
                <Badge v-if="post.pinnedAt" variant="default">置顶</Badge>
                <Badge v-if="post.hiddenAt" variant="destructive" :title="post.hiddenBy ? `由 @${post.hiddenBy.username} 于 ${formatTime(post.hiddenAt)} 隐藏` : formatTime(post.hiddenAt)">
                  {{ hiddenReasonLabel[post.hiddenReason ?? ''] ?? '已隐藏' }}
                </Badge>
                <Badge v-if="post.isAnonymous" variant="secondary">匿名</Badge>
              </div>
            </TableCell>
            <TableCell class="hidden md:table-cell">{{ post.category?.name ?? '—' }}</TableCell>
            <TableCell>
              <NuxtLink v-if="post.author" :to="userPath(post.author.id)" class="hover:underline">@{{ post.author.username }}</NuxtLink>
              <span v-else class="text-muted-foreground italic">{{ post.anonymousName ?? '匿名' }}</span>
            </TableCell>
            <TableCell class="hidden lg:table-cell text-right tabular-nums text-muted-foreground">
              {{ formatNumber(post.viewCount) }} / {{ formatNumber(post.likeCount) }} /
              <NuxtLink :to="{ path: '/comments', query: { postId: post.id } }" class="hover:underline">{{ formatNumber(post.commentCount) }}</NuxtLink>
            </TableCell>
            <TableCell class="hidden md:table-cell whitespace-nowrap">{{ formatTime(post.publishedAt) }}</TableCell>
            <TableCell class="text-right"><AdminRowActions :actions="rowActions(post)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="7" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!items.length" empty-text="还没有已发布的委托"
            @retry="list.refresh" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="篇" @change="list.setPage" />

    <AdminBulkBar :count="selection.size" unit="篇" @clear="select.clear">
      <Button size="sm" variant="outline" @click="act(selectedPosts, 'unhide')"><Eye class="size-4" />恢复</Button>
      <Button size="sm" variant="outline" @click="act(selectedPosts, 'hide')"><EyeOff class="size-4" />隐藏</Button>
      <Button size="sm" variant="destructive" @click="act(selectedPosts, 'delete')"><Trash2 class="size-4" />删除</Button>
    </AdminBulkBar>

    <Dialog :open="!!moving" @update:open="(v) => { if (!v) moving = null }">
      <DialogContent v-if="moving">
        <DialogHeader>
          <DialogTitle>移动《{{ moving.title }}》</DialogTitle>
          <DialogDescription>作者尚未发布的修改也会一并改到新分区。</DialogDescription>
        </DialogHeader>
        <Select v-model="moveTo">
          <SelectTrigger class="w-full"><SelectValue placeholder="选择分区" /></SelectTrigger>
          <SelectContent>
            <SelectItem v-for="c in activeCategories" :key="c.id" :value="c.id">{{ c.name }}{{ c.adminOnly ? '（仅管理员发帖）' : '' }}</SelectItem>
          </SelectContent>
        </Select>
        <DialogFooter>
          <Button variant="outline" @click="moving = null">取消</Button>
          <Button :disabled="!moveTo || moveTo === moving.category?.id" @click="move">移动</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
