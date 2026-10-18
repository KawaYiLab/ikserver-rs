<script setup lang="ts">
import { ExternalLink, Eye, EyeOff, MessageSquare, Trash2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatNumber, formatTime } from '~/lib/format'
import { hiddenReasonLabel } from '~/lib/links'

/** 账号详情的「委托」标签页：此人的实名委托（匿名的不出现，规则 10）。 */
const props = defineProps<{ userId: string }>()
type Post = Schemas['AdminPost']

const siteLink = useSiteLink()
const moderate = useModerate()

const list = useListQuery({
  filters: { postState: 'all' },
  pageKey: 'postPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/posts', {
    params: {
      query: {
        authorId: props.userId,
        visibility: optional<'visible' | 'hidden'>(f.postState),
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading } = list

async function act(post: Post, action: ModerateAction) {
  const done = await moderate({ targetType: 'post', ids: [post.id], action, label: `委托《${post.title}》` })
  if (done) list.refresh()
}

function rowActions(post: Post): RowAction[] {
  return [
    { label: '在主站打开', icon: ExternalLink, href: siteLink(`/post/${post.id}`) },
    { label: '查看评论', icon: MessageSquare, onSelect: () => navigateTo({ path: '/comments', query: { postId: post.id } }) },
    post.hiddenAt
      ? { label: '恢复可见', icon: Eye, onSelect: () => act(post, 'unhide') }
      : { label: '隐藏', icon: EyeOff, destructive: true, onSelect: () => act(post, 'hide') },
    { label: '删除', icon: Trash2, destructive: true, onSelect: () => act(post, 'delete') },
  ]
}
</script>

<template>
  <div>
    <div class="mb-3 flex flex-wrap items-center gap-2">
      <Select v-model="filters.postState">
        <SelectTrigger class="w-36"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem value="visible">公开</SelectItem>
          <SelectItem value="hidden">已隐藏</SelectItem>
        </SelectContent>
      </Select>
      <span class="text-xs text-muted-foreground">只列实名委托；匿名委托不会和账号关联。</span>
      <div class="flex-1" />
      <Button variant="ghost" size="sm" as-child>
        <NuxtLink :to="{ path: '/posts', query: { authorId: userId } }">在帖子管理中打开</NuxtLink>
      </Button>
    </div>
    <div class="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>委托</TableHead>
            <TableHead class="hidden md:table-cell">分区</TableHead>
            <TableHead class="hidden lg:table-cell text-right">浏览 / 赞 / 评论</TableHead>
            <TableHead class="hidden md:table-cell">发布时间</TableHead>
            <TableHead class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="post in result?.items ?? []" :key="post.id">
            <TableCell class="max-w-md">
              <a :href="siteLink(`/post/${post.id}`)" target="_blank" rel="noopener" class="line-clamp-1 font-medium hover:underline">{{ post.title }}</a>
              <div class="mt-1 flex flex-wrap gap-1">
                <Badge v-if="post.pinnedAt">置顶</Badge>
                <Badge v-if="post.hiddenAt" variant="destructive">{{ hiddenReasonLabel[post.hiddenReason ?? ''] ?? '已隐藏' }}</Badge>
              </div>
            </TableCell>
            <TableCell class="hidden md:table-cell">{{ post.category?.name ?? '—' }}</TableCell>
            <TableCell class="hidden lg:table-cell text-right tabular-nums text-muted-foreground">
              {{ formatNumber(post.viewCount) }} / {{ formatNumber(post.likeCount) }} / {{ formatNumber(post.commentCount) }}
            </TableCell>
            <TableCell class="hidden md:table-cell whitespace-nowrap">{{ formatTime(post.publishedAt) }}</TableCell>
            <TableCell class="text-right"><AdminRowActions :actions="rowActions(post)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="5" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" empty-text="还没有发布实名委托"
            @retry="list.refresh" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="篇" @change="list.setPage" />
  </div>
</template>
