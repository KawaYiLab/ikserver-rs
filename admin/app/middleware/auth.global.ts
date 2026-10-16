/** 除登录页外都要求版主或管理员；只给管理员的页面在 `meta.admin` 上声明。 */
export default defineNuxtRouteMiddleware(async (to) => {
  const { load, isStaff, isAdmin } = useSession()
  const me = await load()
  if (to.path === '/login') {
    if (me && isStaff.value) return navigateTo('/')
    return
  }
  if (!me || !isStaff.value) return navigateTo({ path: '/login', query: { next: to.fullPath } })
  if (to.meta.admin && !isAdmin.value) return navigateTo('/')
})
