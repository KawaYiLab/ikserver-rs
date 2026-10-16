import createClient from 'openapi-fetch'
import type { components, paths } from '~/types/api'

export type Schemas = components['schemas']

/** 接口返回的 RFC 9457 Problem（`code` 是稳定的机器码，`detail` 是给人看的中文）。 */
export interface Problem {
  type?: string
  title?: string
  status?: number
  code?: string
  detail?: string
  errors?: { field: string, code: string, detail: string }[]
  requestId?: string
}

export class ApiError extends Error {
  constructor(public status: number, public problem: Problem) {
    super(ApiError.describe(status, problem))
  }

  get code() {
    return this.problem.code
  }

  static describe(status: number, problem: Problem): string {
    const fieldErrors = problem.errors?.map(e => e.detail).filter(Boolean) ?? []
    if (fieldErrors.length) return fieldErrors.join('；')
    if (problem.detail) return problem.detail
    if (status >= 500) return `服务器出错了（${problem.requestId ?? status}）`
    return `请求失败（${status}）`
  }
}

/** 同源调用 `/v1`：网页会话 Cookie 自动带上，CSRF 校验看 Origin。 */
export const api = createClient<paths>({
  baseUrl: typeof window === 'undefined' ? '' : window.location.origin,
  credentials: 'include',
})

let onUnauthorized: (() => void) | undefined

export function setUnauthorizedHandler(handler: () => void) {
  onUnauthorized = handler
}

/** 取出数据；失败时抛 `ApiError`。401 时回到登录页。 */
export async function unwrap<T>(
  call: Promise<{ data?: T, error?: unknown, response: Response }>,
): Promise<T> {
  const { data, error, response } = await call
  if (response.ok) return data as T
  if (response.status === 401) onUnauthorized?.()
  throw new ApiError(response.status, (error ?? {}) as Problem)
}
