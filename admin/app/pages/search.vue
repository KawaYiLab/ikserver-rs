<script setup lang="ts">
import { RefreshCw } from '@lucide/vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatNumber } from '~/lib/format'

definePageMeta({ admin: true })

const { run, failure } = useNotify()
const confirm = useConfirm()
const status = ref<Schemas['SearchStatus'] | null>(null)
const loading = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

async function load() {
  loading.value = true
  try {
    status.value = await unwrap(api.GET('/v1/admin/search'))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
  // 重建中每 5 秒刷新一次。
  clearTimeout(timer)
  if (status.value?.reindexing) timer = setTimeout(load, 5000)
}
onMounted(load)
onBeforeUnmount(() => clearTimeout(timer))

async function reindex() {
  const ok = await confirm({
    title: '重建搜索索引？',
    description: '在后台全量对账：补齐缺失的文档、删掉残留的文档。期间搜索照常可用。',
    confirmLabel: '开始重建',
  })
  if (!ok) return
  const started = await run(() => unwrap(api.POST('/v1/admin/search/reindex')).then(() => true), '已开始重建')
  if (started) load()
}
</script>

<template>
  <div>
    <AdminPageHeader title="搜索索引" description="索引只收录公开内容，数量略少于数据库是正常的。">
      <Button variant="outline" size="sm" :disabled="loading" @click="load">
        <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />刷新
      </Button>
      <Button size="sm" :disabled="!status?.engineConfigured || status.reindexing" @click="reindex">
        {{ status?.reindexing ? '重建中…' : '重建索引' }}
      </Button>
    </AdminPageHeader>

    <template v-if="status">
      <Card v-if="!status.engineConfigured" class="mb-6 border-warning">
        <CardHeader>
          <CardTitle>没有配置搜索引擎</CardTitle>
          <CardDescription>全部搜索走数据库兜底；配置 Meilisearch 后才能重建索引。</CardDescription>
        </CardHeader>
      </Card>
      <Card v-else-if="status.engineError" class="mb-6 border-destructive">
        <CardHeader>
          <CardTitle class="text-destructive">读取索引统计失败</CardTitle>
          <CardDescription class="font-mono text-xs break-all">{{ status.engineError }}</CardDescription>
        </CardHeader>
      </Card>

      <div class="grid gap-4 sm:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              帖子
              <Badge v-if="status.reindexing" variant="secondary">重建中</Badge>
            </CardTitle>
          </CardHeader>
          <CardContent class="grid grid-cols-2 gap-4">
            <div>
              <div class="text-sm text-muted-foreground">索引</div>
              <div class="text-2xl font-semibold tabular-nums">{{ formatNumber(status.indexedPosts) }}</div>
            </div>
            <div>
              <div class="text-sm text-muted-foreground">数据库（已发布）</div>
              <div class="text-2xl font-semibold tabular-nums">{{ formatNumber(status.dbPosts) }}</div>
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle class="flex items-center gap-2">
              评论
              <Badge v-if="status.reindexing" variant="secondary">重建中</Badge>
            </CardTitle>
          </CardHeader>
          <CardContent class="grid grid-cols-2 gap-4">
            <div>
              <div class="text-sm text-muted-foreground">索引</div>
              <div class="text-2xl font-semibold tabular-nums">{{ formatNumber(status.indexedComments) }}</div>
            </div>
            <div>
              <div class="text-sm text-muted-foreground">数据库</div>
              <div class="text-2xl font-semibold tabular-nums">{{ formatNumber(status.dbComments) }}</div>
            </div>
          </CardContent>
        </Card>
      </div>
    </template>
  </div>
</template>
