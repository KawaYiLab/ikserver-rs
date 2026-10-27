import { ApiError, api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { SettingValue, SiteValues } from '~/lib/settings'
import { getPath, sameValue, setPath } from '~/lib/settings'

/**
 * 站点参数的编辑草稿（「系统设置」与「内容与安全」共用）。`paths` 是本页管的项：
 * 只有它们算「未保存的修改」，保存时整份参数一起提交。
 */
export function useSiteSettings(paths: string[]) {
  const { failure, success } = useNotify()
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

  const dirtyPaths = computed(() => {
    if (!settings.value || !draft.value) return []
    return paths.filter(path => !sameValue(getPath(draft.value, path), getPath(settings.value!.values, path)))
  })

  function value(path: string) {
    return getPath(draft.value, path)
  }

  function defaultOf(path: string) {
    return getPath(settings.value?.defaults, path)
  }

  function update(path: string, next: SettingValue) {
    if (!draft.value) return
    setPath(draft.value, path, next)
    // 后端的字段错误可能落在上一级（例如 `ipBans`），一起清掉。
    for (const key of Object.keys(errors.value)) {
      if (path === key || path.startsWith(`${key}.`)) delete errors.value[key]
    }
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

  // 离开页面前提醒还没保存的修改。
  onBeforeRouteLeave(async () => {
    if (!dirtyPaths.value.length) return true
    return confirm({
      title: '放弃未保存的修改？',
      description: `还有 ${dirtyPaths.value.length} 项没有保存。`,
      confirmLabel: '放弃',
      destructive: true,
    })
  })

  return { settings, draft, errors, loading, saving, dirtyPaths, load, value, defaultOf, update, discard, save }
}
