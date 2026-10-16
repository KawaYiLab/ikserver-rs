/** 展示用的格式化。时间一律按北京时间显示。 */
const dateTime = new Intl.DateTimeFormat('zh-CN', {
  timeZone: 'Asia/Shanghai',
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
})

export function formatTime(value?: string | null) {
  if (!value) return '—'
  return dateTime.format(new Date(value))
}

const number = new Intl.NumberFormat('zh-CN')

export function formatNumber(value?: number | null) {
  return value == null ? '—' : number.format(value)
}

export const roleLabel: Record<string, string> = { user: '用户', moderator: '版主', admin: '管理员' }
export const statusLabel: Record<string, string> = { active: '正常', blocked: '已封禁', deleted: '已注销', merged: '已合并' }
export const reasonLabel: Record<string, string> = {
  spam: '垃圾广告 / 刷屏',
  abuse: '辱骂攻击 / 引战',
  porn: '色情低俗',
  illegal: '违法违规',
  privacy: '泄露隐私',
  misinfo: '不实信息',
  plagiarism: '抄袭搬运',
  other: '其他',
}
export const targetTypeLabel: Record<string, string> = { post: '帖子', comment: '评论', user: '用户' }
export const reportStatusLabel: Record<string, string> = { pending: '待处理', reviewing: '复核中', accepted: '成立', rejected: '驳回' }
export const actionLabel: Record<string, string> = {
  reject: '驳回',
  hide_content: '隐藏内容',
  delete_content: '删除内容',
  warn_user: '警告用户',
  block_user: '封禁用户',
}
export const kindLabel: Record<string, string> = { single: '单选', multiple: '多选', boolean: '判断' }
export const cosmeticKindLabel: Record<string, string> = { character: '角色', city: '城市', news: '新闻' }
