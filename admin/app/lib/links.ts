/** 后台内部链接。 */

/** 账号详情页。 */
export function userPath(id: string, tab?: string) {
  return tab ? { path: `/users/${id}`, query: { tab } } : `/users/${id}`
}

/** 隐藏原因的说明。 */
export const hiddenReasonLabel: Record<string, string> = {
  auto_report: '多人举报自动隐藏',
  moderator: '版主隐藏',
}
