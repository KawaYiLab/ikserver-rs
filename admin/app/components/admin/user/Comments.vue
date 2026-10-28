<script setup lang="ts">
import { ExternalLink, Eye, EyeOff, Trash2 } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { RowAction } from '~/lib/actions'
import { formatTime } from '~/lib/format'
import { hiddenReasonLabel } from '~/lib/links'

/** 账号详情的「评论」标签页：此人的实名评论（匿名的不出现，规则 10）。 */
const props = defineProps<{ userId: string }>()
type Comment = Schemas['AdminComment']

const siteLink = useSiteLink()
const moderate = useModerate()

const list = useListQuery({
  filters: { commentState: 'all' },
  pageKey: 'commentPage',
  fetch: (f, page) => unwrap(api.GET('/v1/admin/comments', {
    params: {
      query: {
        authorId: props.userId,
        visibility: optional<'visible' | 'hidden'>(f.commentState),
        page,
        limit: 20,
      },
    },
  })),
})
const { filters, result, loading } = list

async function act(comment: Comment, action: ModerateAction) {
  const done = await moderate({ targetType: 'comment', ids: [comment.id], action, label: '这条评论' })
  if (done) list.refresh()
}

function rowActions(comment: Comment): RowAction[] {
  return [
    { label: '在主站打开', icon: ExternalLink, href: siteLink(`/post/${comment.postId}?comment=${comment.id}`) },
    comment.hiddenAt
      ? { label: '恢复可见', icon: Eye, onSelect: () => act(comment, 'unhide') }
      : { label: '隐藏', icon: EyeOff, destructive: true, onSelect: () => act(comment, 'hide') },
    { label: '删除', icon: Trash2, destructive: true, onSelect: () => act(comment, 'delete') },
  ]
}
</script>

<template>
  <div>
    <div class="mb-3 flex flex-wrap items-center gap-2">
      <Select v-model="filters.commentState">
        <SelectTrigger class="w-36"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部状态</SelectItem>
          <SelectItem value="visible">公开</SelectItem>
          <SelectItem value="hidden">已隐藏</SelectItem>
        </SelectContent>
      </Select>
      <span class="text-xs text-muted-foreground">只列实名评论。</span>
      <div class="flex-1" />
      <Button variant="ghost" size="sm" as-child>
        <NuxtLink :to="{ path: '/comments', query: { authorId: userId } }">在评论管理中打开</NuxtLink>
      </Button>
    </div>
    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>评论</TableHead>
            <TableHead class="hidden md:table-cell">所在委托</TableHead>
            <TableHead class="hidden md:table-cell">时间</TableHead>
            <TableHead class="w-10"><span class="sr-only">操作</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="comment in result?.items ?? []" :key="comment.id">
            <TableCell class="max-w-md whitespace-normal">
              <p class="line-clamp-2 break-words">{{ comment.text || '（只有图片）' }}</p>
              <Badge v-if="comment.hiddenAt" variant="destructive" class="mt-1">{{ hiddenReasonLabel[comment.hiddenReason ?? ''] ?? '已隐藏' }}</Badge>
            </TableCell>
            <TableCell class="hidden md:table-cell max-w-56">
              <NuxtLink :to="{ path: '/comments', query: { postId: comment.postId } }" class="line-clamp-1 hover:underline">{{ comment.postTitle ?? '（已删除）' }}</NuxtLink>
            </TableCell>
            <TableCell class="hidden md:table-cell whitespace-nowrap">{{ formatTime(comment.createdAt) }}</TableCell>
            <TableCell class="text-right"><AdminRowActions :actions="rowActions(comment)" /></TableCell>
          </TableRow>
          <AdminListState
            :colspan="4" :loading="loading" :failed="list.failed.value" :filtered="list.active.value"
            :empty="!result?.items.length" empty-text="还没有实名评论"
            @retry="list.refresh" @reset="list.reset"
          />
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" unit="条" @change="list.setPage" />
  </div>
</template>
