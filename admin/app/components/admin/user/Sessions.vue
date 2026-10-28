<script setup lang="ts">
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatTime, loginMethodLabel } from '~/lib/format'

/** 账号详情的「登录设备」（只有管理员：含 IP）。 */
const props = defineProps<{ userId: string }>()

const { failure } = useNotify()
const sessions = ref<Schemas['SessionView'][] | null>(null)
const loading = ref(false)
const failed = ref(false)

async function load() {
  loading.value = true
  try {
    sessions.value = await unwrap(api.GET('/v1/admin/users/{id}/sessions', { params: { path: { id: props.userId } } }))
    failed.value = false
  }
  catch (error) {
    failed.value = true
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)
</script>

<template>
  <div>
    <p class="mb-3 text-xs text-muted-foreground">当前有效的登录会话，最近活跃的在前。要让对方在所有设备上下线，用页头的「强制下线」。</p>
    <div class="overflow-hidden rounded-xl border bg-card shadow-[var(--shadow-card)]">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>设备</TableHead>
            <TableHead>登录方式</TableHead>
            <TableHead>IP / 位置</TableHead>
            <TableHead class="hidden md:table-cell">登录时间</TableHead>
            <TableHead>最近活跃</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="s in sessions ?? []" :key="s.id">
            <TableCell class="max-w-xs truncate" :title="s.userAgent ?? ''">{{ s.userAgent ?? '未知设备' }}</TableCell>
            <TableCell>{{ loginMethodLabel[s.loginMethod] ?? s.loginMethod }}</TableCell>
            <TableCell>
              <span class="font-mono text-xs">{{ s.ip ?? '—' }}</span>
              <span v-if="s.location" class="ml-1 text-xs text-muted-foreground">{{ s.location }}</span>
            </TableCell>
            <TableCell class="hidden md:table-cell whitespace-nowrap">{{ formatTime(s.createdAt) }}</TableCell>
            <TableCell class="whitespace-nowrap">{{ formatTime(s.lastSeenAt) }}</TableCell>
          </TableRow>
          <AdminListState
            :colspan="5" :loading="loading" :failed="failed" :empty="!sessions?.length"
            empty-text="当前没有登录中的会话" @retry="load"
          />
        </TableBody>
      </Table>
    </div>
  </div>
</template>
