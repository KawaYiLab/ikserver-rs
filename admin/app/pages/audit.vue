<script setup lang="ts">
import { watchDebounced } from '@vueuse/core'
import { RefreshCw } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatTime } from '~/lib/format'

definePageMeta({ admin: true })

/** 审计动作（与后端写入的 `action` 一致）。 */
const auditActions: Record<string, string> = {
  'user.role.set': '修改角色',
  'user.ai_agent.set': '设置 AI 账号',
  'user.exam.pass': '手动通过考试',
  'user.denny.adjust': '调整丁尼',
  'user.exp.grant': '发放经验',
  'user.block': '封禁账号',
  'user.unblock': '解封账号',
  'exam.question.create': '新建题目',
  'exam.question.update': '编辑题目',
  'exam.question.toggle': '启停题目',
  'exam.question.delete': '删除题目',
  'content.category.create': '新建分区',
  'content.category.update': '编辑分区',
  'content.category.delete': '删除分区',
  'content.tag.update': '修改标签',
  'cosmetics.avatar.create': '新建系统头像',
  'cosmetics.avatar.update': '编辑系统头像',
  'cosmetics.avatar.delete': '下架系统头像',
  'cosmetics.card.create': '新建名片',
  'cosmetics.card.update': '编辑名片',
  'cosmetics.card.delete': '下架名片',
  'ai.settings.update': '修改 AI 设置',
  'ai.model.save': '保存模型',
  'ai.model.delete': '删除模型',
  'ai.character.save': '保存角色',
  'ai.character.delete': '删除角色',
  'search.reindex': '重建索引',
}

const auditTargets: Record<string, string> = {
  user: '用户',
  exam_question: '题目',
  category: '分区',
  tag: '标签',
  system_avatar: '系统头像',
  business_card: '名片',
  ai_settings: 'AI 设置',
  ai_model: 'AI 模型',
  ai_character: 'AI 角色',
  search: '搜索',
}

const { failure } = useNotify()
const actorId = ref('')
const action = ref('all')
const targetType = ref('all')
const targetId = ref('')
const page = ref(1)
const result = ref<Schemas['Paged_AuditEntry'] | null>(null)
const loading = ref(false)
const expanded = ref<string | null>(null)

async function load() {
  loading.value = true
  try {
    result.value = await unwrap(api.GET('/v1/admin/audit', {
      params: {
        query: {
          actorId: actorId.value.trim() || undefined,
          action: action.value === 'all' ? undefined : action.value,
          targetType: targetType.value === 'all' ? undefined : targetType.value,
          targetId: targetId.value.trim() || undefined,
          page: page.value,
          limit: 50,
        },
      },
    }))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}

watch([action, targetType], () => {
  page.value = 1
  load()
})
watchDebounced([actorId, targetId], () => {
  page.value = 1
  load()
}, { debounce: 400 })
onMounted(load)

function details(entry: Schemas['AuditEntry']) {
  return JSON.stringify(entry.details, null, 2)
}

function hasDetails(entry: Schemas['AuditEntry']) {
  const value = entry.details as unknown
  return !!value && typeof value === 'object' && Object.keys(value).length > 0
}
</script>

<template>
  <div>
    <AdminPageHeader title="审计日志" description="后台的每一次修改都会记录操作人、时间、来源与变更内容。">
      <Button variant="outline" size="sm" :disabled="loading" @click="load">
        <RefreshCw class="size-4" :class="{ 'animate-spin': loading }" />刷新
      </Button>
    </AdminPageHeader>

    <div class="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
      <Select v-model="action">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部动作</SelectItem>
          <SelectItem v-for="(label, key) in auditActions" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Select v-model="targetType">
        <SelectTrigger class="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部对象</SelectItem>
          <SelectItem v-for="(label, key) in auditTargets" :key="key" :value="key">{{ label }}</SelectItem>
        </SelectContent>
      </Select>
      <Input v-model="targetId" placeholder="对象 ID" />
      <Input v-model="actorId" placeholder="操作人 ID" />
    </div>

    <div class="rounded-md border">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>时间</TableHead>
            <TableHead>操作人</TableHead>
            <TableHead>动作</TableHead>
            <TableHead>对象</TableHead>
            <TableHead class="hidden lg:table-cell">来源 IP</TableHead>
            <TableHead class="text-right">详情</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <template v-for="entry in result?.items ?? []" :key="entry.id">
            <TableRow>
              <TableCell class="whitespace-nowrap">{{ formatTime(entry.createdAt) }}</TableCell>
              <TableCell>
                <template v-if="entry.actorName">@{{ entry.actorName }}</template>
                <span v-else class="text-muted-foreground">{{ entry.actorId ?? '(已删除)' }}</span>
              </TableCell>
              <TableCell>
                {{ auditActions[entry.action] ?? entry.action }}
                <div class="font-mono text-xs text-muted-foreground">{{ entry.action }}</div>
              </TableCell>
              <TableCell>
                {{ auditTargets[entry.targetType] ?? entry.targetType }}
                <span v-if="entry.targetId" class="font-mono text-xs text-muted-foreground"> {{ entry.targetId }}</span>
              </TableCell>
              <TableCell class="hidden lg:table-cell font-mono text-xs" :title="entry.userAgent ?? ''">{{ entry.ip ?? '—' }}</TableCell>
              <TableCell class="text-right">
                <Button v-if="hasDetails(entry)" size="sm" variant="ghost" @click="expanded = expanded === entry.id ? null : entry.id">
                  {{ expanded === entry.id ? '收起' : '展开' }}
                </Button>
              </TableCell>
            </TableRow>
            <TableRow v-if="expanded === entry.id">
              <TableCell colspan="6" class="bg-muted/50">
                <pre class="max-h-80 overflow-auto text-xs whitespace-pre-wrap">{{ details(entry) }}</pre>
                <div v-if="entry.userAgent" class="mt-2 text-xs text-muted-foreground">UA：{{ entry.userAgent }}</div>
              </TableCell>
            </TableRow>
          </template>
          <TableRow v-if="result && result.items.length === 0">
            <TableCell colspan="6" class="py-10 text-center text-muted-foreground">没有记录</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <AdminPager v-if="result" :page="result.page" :limit="result.limit" :total="result.total" @change="(p) => { page = p; load() }" />
  </div>
</template>
