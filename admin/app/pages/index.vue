<script setup lang="ts">
import { Skeleton } from '@/components/ui/skeleton'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber } from '~/lib/format'

const NuxtLink = resolveComponent('NuxtLink')
const { isAdmin } = useSession()

const { failure } = useNotify()
const badges = useBadges()
const data = ref<Schemas['Dashboard'] | null>(null)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    data.value = await unwrap(api.GET('/v1/admin/dashboard'))
    badges.dashboard.value = data.value
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

const examShort = computed(() => data.value && data.value.exam.enabled < data.value.exam.questionCount)
const aiPercent = computed(() => {
  const ai = data.value?.ai
  if (!ai || ai.siteLimit <= 0) return null
  return Math.min(100, Math.round((ai.siteUsedToday / ai.siteLimit) * 100))
})
</script>

<template>
  <div>
    <AdminPageHeader title="工作台" description="今天的数字按北京时间业务日统计。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div v-if="!data" class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <Skeleton v-for="i in 8" :key="i" class="h-28" />
    </div>
    <div v-else class="space-y-6">
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <NuxtLink to="/reports">
          <AdminStatCard
            title="待处理举报" :value="formatNumber(data.reports.pending)"
            :hint="`复核中 ${formatNumber(data.reports.reviewing)}`"
            :tone="data.reports.pending > 0 ? 'warning' : 'default'"
          />
        </NuxtLink>
        <AdminStatCard title="账号" :value="formatNumber(data.users.total)" :hint="`今日新增 ${formatNumber(data.users.newToday)} · 封禁中 ${formatNumber(data.users.blocked)}`" />
        <AdminStatCard title="帖子" :value="formatNumber(data.posts.total)" :hint="`今日发布 ${formatNumber(data.posts.today)}`" />
        <AdminStatCard title="评论" :value="formatNumber(data.comments.total)" :hint="`今日 ${formatNumber(data.comments.today)}`" />
      </div>
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <NuxtLink to="/exam">
          <AdminStatCard
            :title="examShort ? '已启用题目（低于考试所需题数）' : '已启用题目'"
            :value="`${formatNumber(data.exam.enabled)} / ${formatNumber(data.exam.total)}`"
            :hint="`每场考试抽 ${data.exam.questionCount} 题`"
            :tone="examShort ? 'danger' : 'default'"
          />
        </NuxtLink>
        <AdminStatCard
          title="AI 助手" :value="data.ai.globalEnabled ? '已开启' : '已关闭'"
          :tone="data.ai.globalEnabled ? 'default' : 'warning'"
          :hint="aiPercent === null ? `今日全站用量 ${formatNumber(data.ai.siteUsedToday)}（不限）` : `今日全站额度已用 ${aiPercent}%`"
        />
        <component :is="isAdmin ? NuxtLink : 'div'" :to="{ path: '/ai', query: { tab: 'jobs', jobStatus: 'failed' } }">
          <AdminStatCard
            title="AI 失败任务（24 小时）" :value="formatNumber(data.ai.failedLast24h)"
            :tone="data.ai.failedLast24h > 0 ? 'warning' : 'default'"
            hint="在「AI 助手 → 任务」里查看原因"
          />
        </component>
      </div>
    </div>
  </div>
</template>
