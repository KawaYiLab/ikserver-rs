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
    description: '主站顶部的横幅，用户可以关闭。',
    icon: Megaphone,
    fields: [
      { path: 'announcement.enabled', label: '显示公告', kind: 'switch' },
      { path: 'announcement.text', label: '内容', kind: 'textarea', maxLength: 300, placeholder: '例如：今晚 23:00–23:30 停机维护' },
      { path: 'announcement.link', label: '链接', kind: 'text', placeholder: 'https://', maxLength: 500 },
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
    description: '开启后全站只读，版主和管理员不受影响。',
    icon: Construction,
    fields: [
      { path: 'readOnly.enabled', label: '只读维护模式', kind: 'switch' },
      { path: 'readOnly.message', label: '提示文字', help: '留空使用默认提示。', kind: 'text', maxLength: 200, placeholder: '绳网正在维护，暂时只能浏览，请稍后再来' },
    ],
  },
  {
    id: 'registration',
    title: '注册与登录',
    description: '不影响已有账号登录。',
    icon: UserPlus,
    fields: [
      { path: 'registration.open', label: '开放注册', help: '同时停止邮箱注册和米游社建号。', kind: 'switch' },
      { path: 'registration.mihoyoLogin', label: '米游社扫码登录', kind: 'switch' },
    ],
  },
  {
    id: 'exam',
    title: '入站考试参数',
    description: '只对之后开始的考试生效。',
    icon: ClipboardList,
    fields: [
      { path: 'exam.questionCount', label: '每场题数', kind: 'number', min: 1, max: 500, unit: '题' },
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
    description: '按近期互动和发布时间排序。',
    icon: Flame,
    fields: [
      { path: 'hot.enabled', label: '启用热榜', kind: 'switch' },
      { path: 'hot.count', label: '首页展示数', kind: 'number', min: 1, max: 100, unit: '篇' },
      { path: 'hot.feedSize', label: '热门浏览流条数', kind: 'number', min: 1, max: 2000, unit: '条' },
      { path: 'hot.refreshSecs', label: '刷新间隔', kind: 'number', min: 1, max: 3600, unit: '秒' },
      { path: 'hot.gravity', label: '时间衰减指数', help: '越大，旧帖掉得越快。', kind: 'number', min: 0.1, max: 10, step: 0.1 },
      { path: 'hot.recentDays', label: '统计最近几天的互动', kind: 'number', min: 1, max: 90, unit: '天' },
      { path: 'hot.candidates', label: '候选帖数', kind: 'number', min: 1, max: 10000, unit: '篇' },
      { path: 'hot.maxPerAuthor', label: '同一作者最多占几篇', help: '0 为不限。', kind: 'number', min: 0, max: 100, unit: '篇' },
    ],
  },
  {
    id: 'moderation',
    title: '举报与审核',
    description: '被多人举报的内容会先自动隐藏，等待复核。',
    icon: ShieldAlert,
    fields: [
      { path: 'moderation.autoHideThreshold', label: '自动隐藏阈值', help: '多少人举报后自动隐藏。', kind: 'number', min: 1, max: 100, unit: '人' },
    ],
  },
  {
    id: 'rateLimit',
    title: '全站限流',
    description: '每分钟请求次数。写包括发帖、评论、点赞等。',
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
  { path: 'newUser.restrictHours', label: '新账号期限', help: '0 为关闭新账号限制。', kind: 'number', min: 0, max: 720, unit: '小时' },
  { path: 'newUser.postsPerDay', label: '每天最多发帖', help: '0 为不限。', kind: 'number', min: 0, max: 1000, unit: '篇' },
  { path: 'newUser.commentsPerDay', label: '每天最多评论', help: '0 为不限。', kind: 'number', min: 0, max: 10000, unit: '条' },
  { path: 'newUser.allowLinks', label: '允许贴外部链接', kind: 'switch' },
]
