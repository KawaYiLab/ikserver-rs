<script setup lang="ts">
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import {
  formatNumber, formatTime, moderationActionLabel, reasonLabel, reportStatusLabel, reportStatusVariant,
  resolutionLabel, targetTypeLabel,
} from '~/lib/format'

/** 账号详情的「举报与处罚」。匿名内容相关的一律不出现（后端已排除，规则 10）。 */
const props = defineProps<{ userId: string }>()

const { failure } = useNotify()
const siteLink = useSiteLink()
const data = ref<Schemas['UserModeration'] | null>(null)

onMounted(async () => {
  try {
    data.value = await unwrap(api.GET('/v1/admin/users/{id}/moderation', { params: { path: { id: props.userId } } }))
  }
  catch (error) {
    failure(error)
  }
})

function targetLink(type: string, id: string) {
  if (type === 'post') return siteLink(`/post/${id}`)
  if (type === 'user') return siteLink(`/profile/${id}`)
  return undefined
}

const severe = new Set(['block_user', 'delete_content'])
</script>

<template>
  <div v-if="!data" class="space-y-4">
    <div class="grid gap-4 sm:grid-cols-4">
      <Skeleton v-for="i in 4" :key="i" class="h-24" />
    </div>
    <Skeleton class="h-48" />
  </div>
  <div v-else class="space-y-6">
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <AdminStatCard title="收到的举报" :value="formatNumber(data.stats.total)" hint="针对实名内容与账号本身" />
      <NuxtLink to="/reports" class="block">
        <AdminStatCard title="未结案" :value="formatNumber(data.stats.open)" :tone="data.stats.open ? 'warning' : 'default'" class="h-full" />
      </NuxtLink>
      <AdminStatCard title="成立" :value="formatNumber(data.stats.accepted)" :tone="data.stats.accepted ? 'danger' : 'default'" />
      <AdminStatCard title="驳回" :value="formatNumber(data.stats.rejected)" />
    </div>

    <div class="grid gap-6 lg:grid-cols-2">
      <section>
        <h2 class="mb-3 font-medium">处罚与审核记录</h2>
        <ol v-if="data.actions.length" class="relative space-y-4 border-l pl-5">
          <li v-for="(entry, i) in data.actions" :key="i" class="relative">
            <span class="absolute top-1.5 -left-[25px] size-2.5 rounded-full border-2 border-background" :class="severe.has(entry.action) ? 'bg-destructive' : 'bg-muted-foreground'" />
            <div class="flex flex-wrap items-center gap-2 text-sm">
              <span class="font-medium">{{ moderationActionLabel[entry.action] ?? entry.action }}</span>
              <Badge variant="outline">{{ targetTypeLabel[entry.targetType] }}</Badge>
              <span v-if="entry.reportCount" class="text-xs text-muted-foreground">结案 {{ entry.reportCount }} 条举报</span>
            </div>
            <div class="mt-0.5 text-xs text-muted-foreground">
              {{ formatTime(entry.createdAt) }} · {{ entry.actor ? `@${entry.actor.username}` : '系统' }}
              · <span class="font-mono">{{ entry.targetId }}</span>
            </div>
            <p v-if="entry.note" class="mt-1 text-sm whitespace-pre-wrap">{{ entry.note }}</p>
          </li>
        </ol>
        <p v-else class="rounded-md border p-6 text-center text-sm text-muted-foreground">没有处罚记录</p>
      </section>

      <section>
        <h2 class="mb-3 font-medium">最近收到的举报</h2>
        <ul v-if="data.reports.length" class="space-y-3">
          <li v-for="(report, i) in data.reports" :key="i" class="rounded-md border p-3 text-sm">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-medium">{{ reasonLabel[report.reason] ?? report.reason }}</span>
              <Badge variant="outline">{{ targetTypeLabel[report.targetType] }}</Badge>
              <Badge :variant="reportStatusVariant(report.status)">{{ reportStatusLabel[report.status] }}</Badge>
              <span v-if="report.resolution" class="text-xs text-muted-foreground">{{ resolutionLabel[report.resolution] }}</span>
              <span class="ml-auto text-xs text-muted-foreground">{{ formatTime(report.createdAt) }}</span>
            </div>
            <a
              v-if="targetLink(report.targetType, report.targetId)" :href="targetLink(report.targetType, report.targetId)"
              target="_blank" rel="noopener" class="mt-1 line-clamp-1 hover:underline"
            >
              {{ report.preview.title ?? report.preview.excerpt ?? report.preview.username ?? report.targetId }}
            </a>
            <p v-else class="mt-1 line-clamp-2 text-muted-foreground">{{ report.preview.excerpt ?? report.targetId }}</p>
            <p v-if="report.detail" class="mt-1 text-xs text-muted-foreground">举报说明：{{ report.detail }}</p>
          </li>
        </ul>
        <p v-else class="rounded-md border p-6 text-center text-sm text-muted-foreground">没有收到举报</p>
      </section>
    </div>
  </div>
</template>
