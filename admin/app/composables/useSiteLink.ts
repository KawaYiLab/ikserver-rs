import { api, unwrap } from '~/lib/api'

/** 主站（论坛前台）地址，来自 `GET /v1/site`（部署配置 `IK_SITE_URL`）。没有配置时为 null。 */
export function useSiteUrl() {
  const url = useState<string | null>('site-url', () => null)
  const requested = useState<boolean>('site-url-requested', () => false)
  if (import.meta.client && !requested.value) {
    requested.value = true
    unwrap(api.GET('/v1/site'))
      .then((site) => { url.value = site.siteUrl?.replace(/\/+$/, '') ?? null })
      .catch(() => { requested.value = false })
  }
  return url
}

/** 主站上的链接。后台和主站不在同一个域名下，所以拼成绝对地址；没有配置主站地址时返回 undefined。 */
export function useSiteLink() {
  const base = useSiteUrl()
  return (path: string) => base.value ? `${base.value}${path.startsWith('/') ? path : `/${path}`}` : undefined
}
