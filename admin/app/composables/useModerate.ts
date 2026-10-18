import type { Schemas } from '~/lib/api'

export type ModerateAction = Schemas['ContentAction']

export interface ModerateRequest {
  targetType: 'post' | 'comment'
  ids: string[]
  action: ModerateAction
  /** 弹框里显示的对象名，如「委托《标题》」或「3 条评论」。 */
  label: string
}

interface Pending extends ModerateRequest {
  resolve: (result: Schemas['ContentActionResult'] | undefined) => void
}

/** 当前等待填写的处置框（由 `AdminModerateHost` 渲染）。 */
export const pendingModerate = shallowRef<Pending | null>(null)

/**
 * 隐藏 / 恢复 / 删除帖子或评论：弹框填写备注、选择是否通知作者（删除要输入确认），
 * 提交后返回结果；取消或失败时为 undefined。
 */
export function useModerate() {
  return (request: ModerateRequest) => new Promise<Schemas['ContentActionResult'] | undefined>((resolve) => {
    pendingModerate.value?.resolve(undefined)
    pendingModerate.value = { ...request, resolve }
  })
}
