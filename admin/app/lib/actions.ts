import type { Component } from 'vue'

/** 表格行操作菜单（`AdminRowActions`）里的一项。 */
export interface RowAction {
  label: string
  icon?: Component
  /** 危险操作（删除、封禁）：红色，并和前面的项隔开。 */
  destructive?: boolean
  /** 不可用时写明原因，显示在菜单项下方。 */
  disabledReason?: string | false | null
  /** 不显示（如没有权限）。 */
  hidden?: boolean
  /** 外部链接：新标签页打开。 */
  href?: string
  onSelect?: () => void
}
