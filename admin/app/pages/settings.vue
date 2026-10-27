<script setup lang="ts">
import { useEventListener } from '@vueuse/core'
import { GraduationCap } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { ApiError, api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { formatTime } from '~/lib/format'
import type { SettingValue, SiteValues } from '~/lib/settings'
import { getPath, sameValue, setPath, settingSections } from '~/lib/settings'

definePageMeta({ admin: true })

const { run, failure, success } = useNotify()
const confirm = useConfirm()
const settings = ref<Schemas['SiteSettings'] | null>(null)
const draft = ref<SiteValues | null>(null)
const errors = ref<Record<string, string>>({})
const loading = ref(false)
const saving = ref(false)

function adopt(next: Schemas['SiteSettings']) {
  settings.value = next
  draft.value = structuredClone(toRaw(next.values))
  errors.value = {}
}

async function load() {
  loading.value = true
  try {
    adopt(await unwrap(api.GET('/v1/admin/settings')))
  }
  catch (error) {
    failure(error)
  }
  finally {
    loading.value = false
  }
}
onMounted(load)

const allFields = settingSections.flatMap(section => section.fields)
const dirtyPaths = computed(() => {
  if (!settings.value || !draft.value) return []
  return allFields
    .map(field => field.path)
    .filter(path => !sameValue(getPath(draft.value, path), getPath(settings.value!.values, path)))
})
const overriddenCount = computed(() => settings.value?.overridden.length ?? 0)

function value(path: string) {
  return getPath(draft.value, path)
}
function update(path: string, next: SettingValue) {
  if (!draft.value) return
  setPath(draft.value, path, next)
  delete errors.value[path]
}

function discard() {
  if (settings.value) adopt(settings.value)
}

async function save() {
  if (!draft.value || !dirtyPaths.value.length) return
  saving.value = true
  try {
    adopt(await unwrap(api.PUT('/v1/admin/settings', { body: { values: draft.value } })))
    success('已保存，本实例立即生效，其他实例最长 1 分钟内生效')
  }
  catch (error) {
    if (error instanceof ApiError && error.problem.errors?.length) {
      errors.value = Object.fromEntries(error.problem.errors.map(e => [e.field, e.detail]))
    }
    failure(error)
  }
  finally {
    saving.value = false
  }
}

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

// 离开页面前提醒还没保存的修改。
onBeforeRouteLeave(async () => {
  if (!dirtyPaths.value.length) return true
  return confirm({ title: '放弃未保存的修改？', description: `还有 ${dirtyPaths.value.length} 项没有保存。`, confirmLabel: '放弃', destructive: true })
})

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

    <Transition
      enter-active-class="transition duration-150 ease-out" enter-from-class="translate-y-2 opacity-0"
      leave-active-class="transition duration-100 ease-in" leave-to-class="translate-y-2 opacity-0"
    >
      <div v-if="dirtyPaths.length" class="fixed inset-x-0 bottom-4 z-30 flex justify-center px-4">
        <div class="flex w-full max-w-xl items-center gap-3 rounded-xl border bg-popover px-4 py-3 shadow-lg">
          <span class="size-2 shrink-0 rounded-full bg-brand-strong" />
          <span class="flex-1 text-sm">{{ dirtyPaths.length }} 项修改未保存</span>
          <Button variant="ghost" size="sm" :disabled="saving" @click="discard">
            放弃
          </Button>
          <Button size="sm" :disabled="saving" @click="save">
            {{ saving ? '保存中…' : '保存' }}
          </Button>
        </div>
      </div>
    </Transition>
  </div>
</template>
