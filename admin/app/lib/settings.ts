import type { Component } from 'vue'
import { ClipboardList, Flame, Gauge, ShieldAlert, UserPlus } from '@lucide/vue'
import type { Schemas } from '~/lib/api'

export type SiteValues = Schemas['SiteValues']

/** 设置页上的一项参数。`path` 与后端字段错误的 `field` 一致（例如 `hot.count`）。 */
export interface SettingField {
  path: string
  label: string
  help?: string
  kind: 'switch' | 'number'
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

export type SettingValue = boolean | number | string | string[]

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
