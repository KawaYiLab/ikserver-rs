import type { Component } from 'vue'
import {
  Bot, ClipboardList, FileText, Flag, LayoutDashboard, MessagesSquare, Newspaper, Palette, Search, Settings, Smile, Tags,
  Users,
} from '@lucide/vue'

/** 菜单上的待办提示：`count` 显示数字，`alert` 只显示一个警示点。 */
export type BadgeKey = 'reports' | 'aiFailed' | 'examShort'

export interface NavItem {
  to: string
  label: string
  icon: Component
  /** 只有管理员能看到（后端同样只对管理员开放）。 */
  adminOnly?: boolean
  badge?: BadgeKey
  /** Ctrl+K 里额外能匹配的词。 */
  keywords?: string
}

export interface NavGroup {
  label: string
  items: NavItem[]
}

/** 侧边栏与 Ctrl+K 共用的导航表。 */
export const navGroups: NavGroup[] = [
  {
    label: '概览',
    items: [
      { to: '/', label: '工作台', icon: LayoutDashboard, keywords: 'dashboard 仪表盘 首页' },
    ],
  },
  {
    label: '审核',
    items: [
      { to: '/reports', label: '举报处理', icon: Flag, badge: 'reports', keywords: 'report 举报' },
      { to: '/posts', label: '帖子', icon: Newspaper, keywords: 'post 委托 隐藏 删除 置顶' },
      { to: '/comments', label: '评论', icon: MessagesSquare, keywords: 'comment 回复 隐藏 删除' },
      { to: '/users', label: '账号', icon: Users, keywords: 'user 用户 封禁' },
    ],
  },
  {
    label: '站点配置',
    items: [
      { to: '/content', label: '分区与标签', icon: Tags, keywords: 'category tag 分区 标签' },
      { to: '/exam', label: '入站题库', icon: ClipboardList, badge: 'examShort', keywords: 'exam 考试 题目' },
      { to: '/cosmetics', label: '装扮', icon: Palette, keywords: 'avatar card 头像 名片' },
      { to: '/emotes', label: '表情包', icon: Smile, keywords: 'emote emoji 表情' },
    ],
  },
  {
    label: '系统',
    items: [
      { to: '/ai', label: 'AI 助手', icon: Bot, adminOnly: true, badge: 'aiFailed', keywords: 'ai 模型 角色' },
      { to: '/search', label: '搜索索引', icon: Search, adminOnly: true, keywords: 'search meilisearch 索引' },
      { to: '/audit', label: '审计日志', icon: FileText, adminOnly: true, keywords: 'audit log 日志' },
      { to: '/settings', label: '系统设置', icon: Settings, adminOnly: true, keywords: 'settings 设置 开关 入站考试 exam' },
    ],
  },
]

/** 当前账号能看到的导航（去掉空分组）。 */
export function visibleNav(isAdmin: boolean): NavGroup[] {
  return navGroups
    .map(group => ({ ...group, items: group.items.filter(item => isAdmin || !item.adminOnly) }))
    .filter(group => group.items.length > 0)
}

/** 路径对应的导航项（用来显示页面标题、高亮菜单）。 */
export function navItemFor(path: string): NavItem | undefined {
  const items = navGroups.flatMap(group => group.items)
  return items.find(item => item.to === '/' ? path === '/' : path === item.to || path.startsWith(`${item.to}/`))
}
