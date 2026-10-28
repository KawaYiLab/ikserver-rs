import { api, unwrap } from '~/lib/api'

/** 站点是否还没装好（没有管理员）。结果在本次打开后台期间缓存，装好后调用 `markDone`。 */
export function useSetup() {
  const needed = useState<boolean | null>('setup-needed', () => null)

  async function check() {
    if (needed.value === null) {
      try {
        needed.value = (await unwrap(api.GET('/v1/setup'))).needed
      }
      catch {
        return false
      }
    }
    return needed.value
  }

  function markDone() {
    needed.value = false
  }

  return { check, markDone }
}
