<script setup lang="ts">
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatTime } from '~/lib/format'

definePageMeta({ admin: true })

const { run, failure } = useNotify()
const confirm = useConfirm()
const settings = ref<Schemas['SiteSettings'] | null>(null)
const loading = ref(false)
const saving = ref(false)

async function load() {
  loading.value = true
  try {
    settings.value = await unwrap(api.GET('/v1/admin/settings'))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

async function setExamRequired(on: boolean) {
  if (!settings.value || on === settings.value.examRequired) return
  const ok = await confirm(on
    ? {
        title: '开启入站考试？',
        description: '还没通过考试的账号将不能再发帖、评论、私信、签到等，需要先通过考试。已通过的账号不受影响。',
        confirmLabel: '开启',
      }
    : {
        title: '关闭入站考试？',
        description: '所有账号无需考试即可发帖、评论、私信、签到等，等级按经验计算。考试仍可参加，首次通过照常发奖励。',
        confirmLabel: '关闭',
        destructive: true,
      })
  if (!ok) return
  saving.value = true
  try {
    const saved = await run(
      () => unwrap(api.PUT('/v1/admin/settings', { body: { examRequired: on } })),
      on ? '已开启入站考试' : '已关闭入站考试',
    )
    if (saved) settings.value = saved
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <div>
    <AdminPageHeader title="系统设置" description="修改立即生效，不需要重启或让用户重新登录。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <Card class="max-w-xl">
      <CardHeader>
        <CardTitle>入站考试</CardTitle>
        <CardDescription>
          开启时，新账号要先通过入站考试才能发帖、评论、私信、签到等；管理员、版主与 AI 账号不受限制。
          题目在「入站题库」里维护。
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Skeleton v-if="!settings" class="h-6 w-full" />
        <template v-else>
          <label class="flex items-center justify-between gap-4 text-sm">
            <span class="font-medium">要求通过入站考试</span>
            <Switch
              :model-value="settings.examRequired"
              :disabled="saving"
              @update:model-value="setExamRequired"
            />
          </label>
          <p class="mt-3 text-xs text-muted-foreground">
            {{ settings.updatedAt ? `最近修改于 ${formatTime(settings.updatedAt)}` : '从未修改（默认开启）' }}
          </p>
        </template>
      </CardContent>
    </Card>
  </div>
</template>
