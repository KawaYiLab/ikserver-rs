import { ApiError, api, setUnauthorizedHandler, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'

type Me = Schemas['MeView']

/** 当前登录的后台账号。后台只对版主、管理员开放（`role` 来自论坛账号，D-062）。 */
export function useSession() {
  const me = useState<Me | null>('me', () => null)
  const loaded = useState<boolean>('me-loaded', () => false)
  // 头像不在 /v1/me 里，取自己的公开主页；拿不到就显示首字母。
  const avatarUrl = useState<string | null>('me-avatar', () => null)

  const isStaff = computed(() => me.value?.role === 'moderator' || me.value?.role === 'admin')
  const isAdmin = computed(() => me.value?.role === 'admin')

  async function load(force = false) {
    if (loaded.value && !force) return me.value
    try {
      me.value = await unwrap(api.GET('/v1/me'))
      void loadAvatar(me.value.id)
    }
    catch (error) {
      if (!(error instanceof ApiError) || error.status !== 401) throw error
      me.value = null
    }
    loaded.value = true
    return me.value
  }

  /** 与主站一致：自定义头像优先，其次是装备的系统头像。 */
  async function loadAvatar(id: string) {
    try {
      const profile = await unwrap(api.GET('/v1/users/{id}', { params: { path: { id } } }))
      avatarUrl.value = profile.avatar?.thumbUrl ?? profile.equippedAvatar?.image.thumbUrl ?? null
    }
    catch {
      avatarUrl.value = null
    }
  }

  async function login(identifier: string, password: string) {
    await unwrap(api.POST('/v1/auth/login', { body: { identifier, password, client: 'web' } }))
    return load(true)
  }

  async function logout() {
    try {
      await unwrap(api.POST('/v1/auth/logout'))
    }
    finally {
      me.value = null
      avatarUrl.value = null
      await navigateTo('/login')
    }
  }

  return { me, avatarUrl, loaded, isStaff, isAdmin, load, login, logout }
}

/** 任何接口返回 401（会话过期、被踢下线）都回到登录页。 */
export function installUnauthorizedRedirect() {
  const { me } = useSession()
  setUnauthorizedHandler(() => {
    me.value = null
    const route = useRoute()
    if (route.path !== '/login') navigateTo({ path: '/login', query: { next: route.fullPath } })
  })
}
