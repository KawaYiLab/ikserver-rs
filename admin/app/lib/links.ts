/** 后台内部链接。 */

/** 账号页（按 ID 筛出这个账号）。 */
export function userPath(id: string) {
  return { path: '/users', query: { q: id } }
}

/** 隐藏原因的说明。 */
export const hiddenReasonLabel: Record<string, string> = {
  auto_report: '多人举报自动隐藏',
  moderator: '版主隐藏',
}
