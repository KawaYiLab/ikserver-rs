<script setup lang="ts">
import { ExternalLink, Eye, EyeOff, Filter, Trash2, X } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatNumber, formatTime } from '~/lib/format'
import { hiddenReasonLabel, userPath } from '~/lib/links'

type Comment = Schemas['AdminComment']

const siteLink = useSiteLink()
const moderate = useModerate()

const list = useListQuery({
  filters: { q: '', visibility: 'all', postId: '', authorId: '' },
  text: ['q'],
  fetch: (f, page) => unwrap(api.GET('/v1/admin/comments', {
    params: {
      query: {
        q: optional(f.q),
        visibility: optional<'visible' | 'hidden'>(f.visibility),
        postId: optional(f.postId),
        authorId: optional(f.authorId),
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading, selection } = list
const select = useSelection(selection, () => result.value?.items.map(item => item.id) ?? [])
const items = computed(() => result.value?.items ?? [])
const selected = computed(() => items.value.filter(c => selection.value.has(c.id)))
const applied = computed(() => list.applied.value.filters)

const postTitle = computed(() => items.value.find(c => c.postId === applied.value.postId)?.postTitle ?? applied.value.postId)
const authorName = computed(() => items.value.find(c => c.author?.id === applied.value.authorId)?.author?.username ?? applied.value.authorId)

function commentUrl(comment: Comment) {
  return siteLink(`/post/${comment.postId}?comment=${comment.id}`)
}

async function act(comments: Comment[], action: ModerateAction) {
  const label = comments.length === 1 ? '这条评论' : ` ${comments.length} 条评论`
  const done = await moderate({ targetType: 'comment', ids: comments.map(c => c.id), action, label })
  if (done) list.refresh()
}

function rowActions(comment: Comment): RowAction[] {
  return [
    { label: '在主站打开', icon: ExternalLink, href: commentUrl(comment) },
    { label: '只看这篇委托的评论', icon: Filter, hidden: applied.value.postId === comment.postId, onSelect: () => { filters.postId = comment.postId } },
    comment.hiddenAt
      ? { label: '恢复可见', icon: Eye, onSelect: () => act([comment], 'unhide') }
      : { label: '隐藏', icon: EyeOff, destructive: true, onSelect: () => act([comment], 'hide') },
    { label: '删除', icon: Trash2, destructive: true, onSelect: () => act([comment], 'delete') },
  ]
}
</script>

<template>
  <div>
    <AdminPageHeader title="评论" description="直接隐藏、恢复或删除评论。">
      <AdminRefreshButton :loading="loading" @click="list.refresh" />
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-3">
      <Input v-model="filters.q" placeholder="评论内容 / 评论 ID" class="sm:col-span-2" />
      <Select v-model="filters.visibility">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem value="visible">公开</SelectItem>
          <SelectItem value="hidden">已隐藏</SelectItem>
        </SelectContent>
      </Select>
    </div>
    <div v-if="applied.postId || applied.authorId" class="mb-4 flex flex-wrap items-center gap-2 text-sm">
      <Badge v-if="applied.postId" variant="secondary" class="gap-1 py-1">
        委托：{{ postTitle }}
        <button type="button" aria-label="取消按委托筛选" class="rounded-full hover:bg-background/60" @click="filters.postId = ''">
          <X class="size-3" />
        </button>
      </Badge>
      <Badge v-if="applied.authorId" variant="secondary" class="gap-1 py-1">
        作者：@{{ authorName }}
        <button type="button" aria-label="取消按作者筛选" class="rounded-full hover:bg-background/60" @click="filters.authorId = ''">
          <X class="size-3" />
        </button>
      </Badge>
      <span v-if="applied.authorId" class="text-xs text-muted-foreground">按作者筛选时不包含匿名评论</span>
    </div>

    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-10">
              <Checkbox :model-value="select.all.value" aria-label="全选本页" @update:model-value="select.toggleAll" />
            </TableHead>
            <TableHead>评论</TableHead>
            <TableHead class="hidden md:table-cell">所在委托</TableHead>
            <TableHead>作者</TableHead>
            <TableHead class="hidden lg:table-cell text-right">赞</TableHead>
            <TableHead class="hidden md:table-cell">时间</TableHead>
            <TableHead class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="comment in items" :key="comment.id" :data-state="select.has(comment.id) ? 'selected' : undefined">
            <TableCell class="align-top">
              <Checkbox :model-value="select.has(comment.id)" aria-label="选择" @update:model-value="(v) => select.toggle(comment.id, v)" />
            </TableCell>
            <TableCell class="max-w-md whitespace-normal">
              <p class="line-clamp-3 break-words" :class="{ 'text-muted-foreground': comment.hiddenAt }">{{ comment.text || '（只有图片）' }}</p>
              <div v-if="comment.images.length" class="mt-2 flex gap-1">
                <a v-for="image in comment.images.slice(0, 4)" :key="image.id" :href="image.url" target="_blank" rel="noopener">
                  <img :src="image.thumbUrl" alt="" class="size-12 rounded object-cover">
                </a>
                <span v-if="comment.images.length > 4" class="self-end text-xs text-muted-foreground">+{{ comment.images.length - 4 }}</span>
              </div>
              <div class="mt-1 flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
                <span class="font-mono">{{ comment.id }}</span>
                <span>· #{{ comment.floor }}</span>
                <Badge v-if="comment.pinned" variant="default">置顶</Badge>
                <Badge v-if="comment.accepted" variant="default">已采纳</Badge>
                <Badge v-if="comment.hiddenAt" variant="destructive" :title="comment.hiddenBy ? `由 @${comment.hiddenBy.username} 隐藏` : ''">
                  {{ hiddenReasonLabel[comment.hiddenReason ?? ''] ?? '已隐藏' }}
                </Badge>
                <Badge v-if="comment.isAnonymous" variant="secondary">匿名</Badge>
              </div>
            </TableCell>
            <TableCell class="hidden md:table-cell max-w-56">
              <button v-if="comment.postTitle" type="button" class="line-clamp-2 text-left hover:underline" @click="filters.postId = comment.postId">
                {{ comment.postTitle }}
              </button>
              <span v-else class="text-muted-foreground">（已删除）</span>
            </TableCell>
            <TableCell>
              <NuxtLink v-if="comment.author" :to="userPath(comment.author.id)" class="hover:underline">@{{ comment.author.username }}</NuxtLink>
              <span v-else class="text-muted-foreground italic">{{ comment.anonymousName ?? '匿名' }}</span>
            </TableCell>
            <TableCell class="hidden lg:table-cell text-right tabular-nums">{{ formatNumber(comment.likeCount) }}</TableCell>
            <TableCell class="hidden md:table-cell whitespace-nowrap">{{ formatTime(comment.createdAt) }}</TableCell>
            <TableCell class="text-right"><AdminRowActions :actions="rowActions(comment)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="7" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!items.length" empty-text="还没有评论"
            @retry="list.refresh" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="条" @change="list.setPage" />

    <AdminBulkBar :count="selection.size" unit="条" @clear="select.clear">
      <Button size="sm" variant="outline" @click="act(selected, 'unhide')"><Eye class="size-4" />恢复</Button>
      <Button size="sm" variant="outline" @click="act(selected, 'hide')"><EyeOff class="size-4" />隐藏</Button>
      <Button size="sm" variant="destructive" @click="act(selected, 'delete')"><Trash2 class="size-4" />删除</Button>
    </AdminBulkBar>
  </div>
</template>
