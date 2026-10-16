export interface ConfirmOptions {
  title: string
  description?: string
  /** 确认按钮上的字，默认「确认」。 */
  confirmLabel?: string
  destructive?: boolean
  /** 高危操作：要求输入这段文字才能确认。 */
  typeToConfirm?: string
}

interface Pending extends ConfirmOptions {
  resolve: (ok: boolean) => void
}

/** 当前等待回答的确认框（由 `AdminConfirmHost` 渲染；后台是纯 SPA，模块级状态即可）。 */
export const pendingConfirm = shallowRef<Pending | null>(null)

/** 弹出确认框，返回用户是否确认。 */
export function useConfirm() {
  return (options: ConfirmOptions) => new Promise<boolean>((resolve) => {
    pendingConfirm.value?.resolve(false)
    pendingConfirm.value = { ...options, resolve }
  })
}
