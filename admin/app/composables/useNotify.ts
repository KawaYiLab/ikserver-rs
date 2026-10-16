import { toast } from 'vue-sonner'
import { ApiError } from '~/lib/api'

/** 统一的成功 / 失败提示。 */
export function useNotify() {
  function success(message: string) {
    toast.success(message)
  }

  function failure(error: unknown, fallback = '操作失败') {
    const message = error instanceof ApiError ? error.message : error instanceof Error ? error.message : fallback
    toast.error(message)
  }

  /** 执行一个写操作：成功时提示，失败时提示并返回 undefined。 */
  async function run<T>(action: () => Promise<T>, done?: string): Promise<T | undefined> {
    try {
      const result = await action()
      if (done) success(done)
      return result
    }
    catch (error) {
      failure(error)
      return undefined
    }
  }

  return { success, failure, run }
}
