import { useDocumentVisibility, useIntervalFn } from '@vueuse/core'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import type { BadgeKey } from '~/lib/nav'

const REFRESH_MS = 60_000

export interface BadgeValue {
  /** 显示的数字；为 0 时不显示。 */
  count?: number
  /** 只显示警示点（如题库题目不够）。 */
  alert?: boolean
}

/**
 * 菜单上的待办数，取自仪表盘接口。整个后台共用一份：每分钟刷新一次，标签页不可见时暂停，
 * 回到前台立刻刷新；写操作之后可以调用 `refresh()` 立即更新。
 */
export function useBadges() {
  const dashboard = useState<Schemas['Dashboard'] | null>('badges-dashboard', () => null)
  const started = useState<boolean>('badges-started', () => false)
  const visibility = useDocumentVisibility()

  async function refresh() {
    try {
      dashboard.value = await unwrap(api.GET('/v1/admin/dashboard'))
    }
    catch {
      // 徽标只是提示：取不到时保留旧值，不打扰正在做的事。
    }
  }

  if (!started.value && import.meta.client) {
    started.value = true
    const { pause, resume } = useIntervalFn(refresh, REFRESH_MS, { immediateCallback: true })
    watch(visibility, (state) => {
      if (state === 'visible') {
        refresh()
        resume()
      }
      else {
        pause()
      }
    })
  }

  const badges = computed<Record<BadgeKey, BadgeValue>>(() => {
    const d = dashboard.value
    return {
      reports: { count: d?.reports.pending ?? 0 },
      aiFailed: { count: d?.ai.failedLast24h ?? 0 },
      examShort: { alert: !!d && d.exam.enabled < d.exam.questionCount },
    }
  })

  return { dashboard, badges, refresh }
}
