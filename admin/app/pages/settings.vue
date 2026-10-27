<script setup lang="ts">
import { useEventListener } from '@vueuse/core'
import { GraduationCap } from '@lucide/vue'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { api, unwrap } from '~/lib/api'
import { formatTime } from '~/lib/format'
import { getPath, settingSections } from '~/lib/settings'

definePageMeta({ admin: true })

const { run } = useNotify()
const confirm = useConfirm()
const { settings, draft, errors, loading, saving, dirtyPaths, load, value, update, discard, save }
  = useSiteSettings(settingSections.flatMap(section => section.fields.map(field => field.path)))
const overriddenCount = computed(() => settings.value?.overridden.length ?? 0)

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
    if (saved) {
      // 只更新开关，不动还没保存的其他修改。
      settings.value = saved
    }
  }
  finally {
    saving.value = false
  }
}

const navItems = [{ id: 'access', title: '入站考试', icon: GraduationCap }, ...settingSections]
const active = ref('access')
function jump(id: string) {
  document.getElementById(`section-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}
// 左侧分组跟随滚动：取顶部已经越过顶栏的最后一个分组。
useEventListener('scroll', () => {
  let current = navItems[0]!.id
  for (const item of navItems) {
    const top = document.getElementById(`section-${item.id}`)?.getBoundingClientRect().top
    if (top !== undefined && top < 140) current = item.id
  }
  active.value = current
}, { passive: true })
</script>

<template>
  <div class="pb-24">
    <AdminPageHeader title="系统设置" description="缺省值来自部署配置；在这里改过的项以这里为准，不需要重启。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div class="grid gap-6 lg:grid-cols-[13rem_minmax(0,1fr)]">
      <nav class="hidden lg:block" aria-label="设置分组">
        <div class="sticky top-20 space-y-0.5">
          <button
            v-for="section in navItems" :key="section.id"
            type="button"
            :class="cn(
              'flex h-9 w-full items-center gap-2.5 rounded-md px-3 text-left text-sm transition-colors hover:bg-accent',
              active === section.id ? 'bg-accent font-medium' : 'text-muted-foreground',
            )"
            @click="jump(section.id)"
          >
            <component :is="section.icon" class="size-4" />{{ section.title }}
          </button>
          <p v-if="settings" class="px-3 pt-4 text-xs text-muted-foreground">
            {{ overriddenCount ? `共 ${overriddenCount} 项与部署配置不同` : '全部与部署配置一致' }}
          </p>
        </div>
      </nav>

      <div class="min-w-0 space-y-6">
        <Card id="section-access" class="scroll-mt-20 gap-0 py-0 shadow-xs">
          <CardHeader class="border-b py-5">
            <CardTitle class="flex items-center gap-2 text-base">
              <GraduationCap class="size-4 text-muted-foreground" />入站考试
            </CardTitle>
            <CardDescription>
              开启时，新账号要先通过入站考试才能发帖、评论、私信、签到等；管理员、版主与 AI 账号不受限制。题目在「入站题库」里维护。
            </CardDescription>
          </CardHeader>
          <CardContent class="py-4">
            <Skeleton v-if="!settings" class="h-6 w-full" />
            <template v-else>
              <label class="flex items-center justify-between gap-4 text-sm">
                <span class="font-medium">要求通过入站考试</span>
                <Switch :model-value="settings.examRequired" :disabled="saving" @update:model-value="setExamRequired" />
              </label>
              <p class="mt-2 text-xs text-muted-foreground">
                单独保存，切换时会再确认一次。{{ settings.updatedAt ? `设置最近修改于 ${formatTime(settings.updatedAt)}` : '' }}
              </p>
            </template>
          </CardContent>
        </Card>

        <Card
          v-for="section in settingSections" :id="`section-${section.id}`" :key="section.id"
          class="scroll-mt-20 gap-0 py-0 shadow-xs"
        >
          <CardHeader class="border-b py-5">
            <CardTitle class="flex items-center gap-2 text-base">
              <component :is="section.icon" class="size-4 text-muted-foreground" />{{ section.title }}
            </CardTitle>
            <CardDescription>{{ section.description }}</CardDescription>
          </CardHeader>
          <CardContent class="divide-y py-0">
            <template v-if="draft && settings">
              <AdminSettingField
                v-for="field in section.fields" :key="field.path"
                :field="field"
                :model-value="value(field.path)"
                :default-value="getPath(settings.defaults, field.path)"
                :dirty="dirtyPaths.includes(field.path)"
                :error="errors[field.path]"
                :disabled="saving"
                @update:model-value="update(field.path, $event)"
              />
            </template>
            <div v-else class="space-y-3 py-4">
              <Skeleton v-for="i in section.fields.length" :key="i" class="h-9 w-full" />
            </div>
          </CardContent>
        </Card>
      </div>
    </div>

    <AdminSaveBar :count="dirtyPaths.length" :saving="saving" @save="save" @discard="discard" />
  </div>
</template>
