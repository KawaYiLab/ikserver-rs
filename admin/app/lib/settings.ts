import type { Component } from 'vue'
import { ClipboardList, Construction, Flame, Gauge, Megaphone, ShieldAlert, UserPlus } from '@lucide/vue'
import type { Schemas } from '~/lib/api'

export type SiteValues = Schemas['SiteValues']

/** 设置页上的一项参数。`path` 与后端字段错误的 `field` 一致（例如 `hot.count`）。 */
export interface SettingField {
  path: string
  label: string
  help?: string
  kind: 'switch' | 'number' | 'text' | 'textarea' | 'select' | 'datetime'
  /** `select` 的选项。 */
  options?: { value: string, label: string }[]
  placeholder?: string
  /** 文本最长字数。 */
  maxLength?: number
  min?: number
  max?: number
  step?: number
  unit?: string
}

export interface SettingSection {
  id: string
  title: string
  description: string
  icon: Component
  fields: SettingField[]
}

/** 「系统设置」的分组。取值范围与后端 `SiteValues::validate` 一致。 */
export const settingSections: SettingSection[] = [
  {
    id: 'announcement',
    title: '站点公告',
    description: '显示在主站顶部的横幅，用户可以关掉；内容或链接改了会重新出现。',
    icon: Megaphone,
    fields: [
      { path: 'announcement.enabled', label: '显示公告', kind: 'switch' },
      { path: 'announcement.text', label: '内容', kind: 'textarea', maxLength: 300, placeholder: '例如：今晚 23:00–23:30 停机维护' },
      { path: 'announcement.link', label: '链接', help: '点击公告时打开，留空则不跳转。', kind: 'text', placeholder: 'https://', maxLength: 500 },
      {
        path: 'announcement.tone', label: '样式', kind: 'select',
        options: [{ value: 'info', label: '普通' }, { value: 'warning', label: '醒目' }],
      },
      { path: 'announcement.startsAt', label: '开始显示', help: '留空为立即。', kind: 'datetime' },
      { path: 'announcement.endsAt', label: '结束显示', help: '留空为一直显示。', kind: 'datetime' },
    ],
  },
  {
    id: 'readOnly',
    title: '维护模式',
    description: '开启后全站只读：普通用户与游客不能发帖、评论、私信等，版主与管理员不受影响。',
    icon: Construction,
    fields: [
      { path: 'readOnly.enabled', label: '只读维护模式', kind: 'switch' },
      { path: 'readOnly.message', label: '提示文字', help: '用户尝试写操作时看到；留空用默认提示。', kind: 'text', maxLength: 200, placeholder: '绳网正在维护，暂时只能浏览，请稍后再来' },
    ],
  },
  {
    id: 'registration',
    title: '注册与登录',
    description: '关闭后已有账号不受影响，照常登录。',
    icon: UserPlus,
    fields: [
      { path: 'registration.open', label: '开放注册', help: '关闭时邮箱注册与米游社首次登录建号都会被拒绝。', kind: 'switch' },
      { path: 'registration.mihoyoLogin', label: '米游社扫码登录', help: '关闭时扫码登录与绑定不可用，已有的绑定照常展示与解绑。', kind: 'switch' },
    ],
  },
  {
    id: 'exam',
    title: '入站考试参数',
    description: '只影响之后开始的考试，进行中的考试按开考时的参数。',
    icon: ClipboardList,
    fields: [
      { path: 'exam.questionCount', label: '每场题数', help: '题库启用的题目不够时取全部。', kind: 'number', min: 1, max: 500, unit: '题' },
      { path: 'exam.passScorePercent', label: '及格线', kind: 'number', min: 1, max: 100, unit: '%' },
      { path: 'exam.timeLimitSecs', label: '答题时限', kind: 'number', min: 60, max: 86400, step: 60, unit: '秒' },
      { path: 'exam.maxFailsBeforeCooldown', label: '连续失败几次后冷却', kind: 'number', min: 1, max: 100, unit: '次' },
      { path: 'exam.failCooldownSecs', label: '冷却时长', kind: 'number', min: 0, max: 604800, step: 60, unit: '秒' },
      { path: 'exam.rewardDenny', label: '首次通过奖励丁尼', kind: 'number', min: 0, max: 100000 },
      { path: 'exam.rewardExp', label: '首次通过奖励经验', kind: 'number', min: 0, max: 100000 },
    ],
  },
  {
    id: 'hot',
    title: '热榜',
    description: '按近期互动与时间衰减排序，定时重新计算。',
    icon: Flame,
    fields: [
      { path: 'hot.enabled', label: '启用热榜', help: '关闭后首页不再插入热门帖，热门浏览流为空。', kind: 'switch' },
      { path: 'hot.count', label: '首页展示数', kind: 'number', min: 1, max: 100, unit: '篇' },
      { path: 'hot.feedSize', label: '热门浏览流条数', kind: 'number', min: 1, max: 2000, unit: '条' },
      { path: 'hot.refreshSecs', label: '刷新间隔', kind: 'number', min: 1, max: 3600, unit: '秒' },
      { path: 'hot.gravity', label: '时间衰减指数', help: '越大，旧帖掉得越快。', kind: 'number', min: 0.1, max: 10, step: 0.1 },
      { path: 'hot.recentDays', label: '统计最近几天的互动', kind: 'number', min: 1, max: 90, unit: '天' },
      { path: 'hot.candidates', label: '候选帖数', help: '按最近活跃取这么多篇参与排序。', kind: 'number', min: 1, max: 10000, unit: '篇' },
      { path: 'hot.maxPerAuthor', label: '同一作者最多占几篇', help: '0 表示不限，匿名帖不参与。', kind: 'number', min: 0, max: 100, unit: '篇' },
    ],
  },
  {
    id: 'moderation',
    title: '举报与审核',
    description: '自动隐藏的内容会进入「举报处理」的复核中，驳回举报时恢复。',
    icon: ShieldAlert,
    fields: [
      { path: 'moderation.autoHideThreshold', label: '自动隐藏阈值', help: '同一内容收到这么多个不同的人的待处理举报时自动隐藏。', kind: 'number', min: 1, max: 100, unit: '人' },
    ],
  },
  {
    id: 'rateLimit',
    title: '全站限流',
    description: '每分钟请求次数，读 = 浏览，写 = 发帖、评论、点赞等。改动后计数从零开始。',
    icon: Gauge,
    fields: [
      { path: 'rateLimit.enabled', label: '启用全站限流', kind: 'switch' },
      { path: 'rateLimit.anonReadPerMin', label: '游客读', kind: 'number', min: 1, max: 100000, unit: '次/分' },
      { path: 'rateLimit.anonWritePerMin', label: '游客写', kind: 'number', min: 1, max: 100000, unit: '次/分' },
      { path: 'rateLimit.userReadPerMin', label: '登录用户读', kind: 'number', min: 1, max: 100000, unit: '次/分' },
      { path: 'rateLimit.userWritePerMin', label: '登录用户写', kind: 'number', min: 1, max: 100000, unit: '次/分' },
    ],
  },
]

// string 要排在 boolean 前面：Vue 对同时允许两者的 prop 会把空字符串转成 true。
export type SettingValue = string | number | boolean | string[] | null | unknown[]

export function getPath(values: unknown, path: string): SettingValue {
  return path.split('.').reduce<any>((node, key) => node?.[key], values) as SettingValue
}

export function setPath(values: unknown, path: string, value: SettingValue) {
  const keys = path.split('.')
  const last = keys.pop()!
  const parent = keys.reduce<any>((node, key) => node[key], values)
  parent[last] = value
}

export function sameValue(a: SettingValue, b: SettingValue) {
  return JSON.stringify(a) === JSON.stringify(b)
}

/** 「内容与安全」页的新账号限制。取值范围与后端一致。 */
export const newUserFields: SettingField[] = [
  { path: 'newUser.restrictHours', label: '新账号期限', help: '注册后这么久以内按新账号限制；0 关闭下面全部限制。', kind: 'number', min: 0, max: 720, unit: '小时' },
  { path: 'newUser.postsPerDay', label: '每天最多发帖', help: '0 表示不限。', kind: 'number', min: 0, max: 1000, unit: '篇' },
  { path: 'newUser.commentsPerDay', label: '每天最多评论', help: '0 表示不限。', kind: 'number', min: 0, max: 10000, unit: '条' },
  { path: 'newUser.allowLinks', label: '允许贴外部链接', help: '关闭时新账号的帖子与评论里不能出现站外链接。', kind: 'switch' },
]
