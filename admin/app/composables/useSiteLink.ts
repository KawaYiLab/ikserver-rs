/** 主站（论坛前台）上的链接。后台和主站不在同一个域名下，所以拼成绝对地址。 */
export function useSiteLink() {
  const base = useRuntimeConfig().public.siteUrl.replace(/\/+$/, '')
  return (path: string) => `${base}${path.startsWith('/') ? path : `/${path}`}`
}
