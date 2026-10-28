/** 除登录页和首次安装页外都要求版主或管理员；只给管理员的页面在 `meta.admin` 上声明。 */
export default defineNuxtRouteMiddleware(async (to) => {
  if (to.path === '/setup') return
  const { load, isStaff, isAdmin } = useSession()
  const me = await load()
  if (to.path === '/login') {
    if (me && isStaff.value) return navigateTo('/')
    if (await useSetup().check()) return navigateTo('/setup')
    return
  }
  if (!me || !isStaff.value) return navigateTo({ path: '/login', query: { next: to.fullPath } })
  if (to.meta.admin && !isAdmin.value) return navigateTo('/')
})
