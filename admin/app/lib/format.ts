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

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline'

/** 账号状态徽标：封禁标红，注销 / 合并灰色。 */
export function userStatusVariant(status: string): BadgeVariant {
  return status === 'active' ? 'outline' : status === 'blocked' ? 'destructive' : 'secondary'
}

export function roleVariant(role: string): BadgeVariant {
  return role === 'user' ? 'outline' : 'default'
}

/** 举报状态徽标：待处理标红。 */
export function reportStatusVariant(status: string): BadgeVariant {
  return status === 'pending' ? 'destructive' : status === 'reviewing' ? 'secondary' : 'outline'
}

/** 后台任务状态徽标：失败标红。 */
export function jobStatusVariant(status: string): BadgeVariant {
  return status === 'failed' ? 'destructive' : status === 'completed' ? 'outline' : 'secondary'
}

export const dennyActionLabel: Record<string, string> = {
  check_in: '签到',
  gift_sent: '投币',
  gift_received: '收到投币',
  spend_rename: '改名',
  spend_avatar: '设置头像',
  exam_passed: '通过入站考试',
  admin_grant: '管理员发放',
  admin_deduct: '管理员扣除',
  spend_delete_post: '删帖扣除',
  refund: '退还',
  level_up: '升级奖励',
}

export const expActionLabel: Record<string, string> = {
  check_in: '签到',
  create_post: '发帖',
  create_comment: '评论',
  like_give: '点赞',
  comment_accepted: '评论被采纳',
  denny_received: '收到投币',
  exam_passed: '通过入站考试',
  admin_grant: '管理员发放',
  like_received: '被点赞',
  comment_received: '被评论',
  favorite_received: '被收藏',
}

export const expBucketLabel: Record<string, string> = { active: '主动', passive: '被动', exempt: '不计上限' }

/** 审核记录的动作。 */
export const moderationActionLabel: Record<string, string> = {
  auto_hide: '多人举报自动隐藏',
  reject: '驳回举报',
  hide_content: '隐藏内容',
  unhide_content: '恢复内容',
  delete_content: '删除内容',
  warn_user: '警告',
  block_user: '封禁',
  unblock_user: '解封',
}

export const resolutionLabel: Record<string, string> = {
  none: '不处理',
  content_hidden: '隐藏内容',
  content_deleted: '删除内容',
  user_warned: '警告用户',
  user_blocked: '封禁用户',
}

export const loginMethodLabel: Record<string, string> = {
  password: '密码',
  email_code: '邮箱验证码',
  register: '注册',
  mihoyo: '米游社扫码',
  password_reset: '重置密码',
  password_change: '修改密码',
}

const relative = new Intl.RelativeTimeFormat('zh-CN', { numeric: 'auto' })

/** 「3 分钟前」「昨天」；超过 7 天显示日期时间。 */
export function formatRelative(value?: string | null, now = Date.now()) {
  if (!value) return '—'
  const seconds = Math.round((new Date(value).getTime() - now) / 1000)
  const abs = Math.abs(seconds)
  if (abs < 60) return '刚刚'
  if (abs < 3600) return relative.format(Math.round(seconds / 60), 'minute')
  if (abs < 86400) return relative.format(Math.round(seconds / 3600), 'hour')
  if (abs < 7 * 86400) return relative.format(Math.round(seconds / 86400), 'day')
  return formatTime(value)
}
