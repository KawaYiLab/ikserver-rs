<script setup lang="ts">
import { onKeyStroke, useColorMode, watchDebounced } from '@vueuse/core'
import { CornerDownLeft, Loader2, Moon, Search, User } from '@lucide/vue'
import { ListboxFilter } from 'reka-ui'
import { Command, CommandGroup, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Badge } from '@/components/ui/badge'
import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'
import { roleLabel, statusLabel } from '~/lib/format'
import { visibleNav } from '~/lib/nav'
import type { NavItem } from '~/lib/nav'

/**
 * 全局命令面板（Ctrl+K / ⌘K）：跳转页面、按用户名或 ID 找账号、切换主题。
 * 过滤由这里自己做（账号是异步搜出来的），所以不用 CommandInput 自带的文本过滤。
 */
const open = defineModel<boolean>('open', { default: false })

const { isAdmin } = useSession()
const mode = useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
const query = ref('')
const users = ref<Schemas['AdminUser'][]>([])
const searching = ref(false)
let seq = 0

onKeyStroke(['k', 'K'], (event) => {
  if (!(event.ctrlKey || event.metaKey)) return
  event.preventDefault()
  open.value = !open.value
})

watch(open, (value) => {
  if (!value) {
    query.value = ''
    users.value = []
  }
})

const keyword = computed(() => query.value.trim())

const pages = computed(() => {
  const all = visibleNav(isAdmin.value).flatMap(group => group.items.map(item => ({ ...item, group: group.label })))
  const k = keyword.value.toLowerCase()
  if (!k) return all
  return all.filter(item => `${item.label} ${item.group} ${item.keywords ?? ''}`.toLowerCase().includes(k))
})

watchDebounced(keyword, async (value) => {
  const current = ++seq
  if (!value) {
    users.value = []
    searching.value = false
    return
  }
  searching.value = true
  try {
    const page = await unwrap(api.GET('/v1/admin/users', { params: { query: { q: value, limit: 6 } } }))
    if (current === seq) users.value = page.items
  }
  catch {
    if (current === seq) users.value = []
  }
  finally {
    if (current === seq) searching.value = false
  }
}, { debounce: 250 })

async function go(to: string | { path: string, query?: Record<string, string> }) {
  open.value = false
  await navigateTo(to)
}

function openPage(item: NavItem) {
  go(item.to)
}

function openUser(user: Schemas['AdminUser']) {
  go({ path: '/users', query: { q: user.id } })
}

function toggleTheme() {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
  open.value = false
}

const nothing = computed(() => !!keyword.value && !searching.value && pages.value.length === 0 && users.value.length === 0)
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="top-[20%] translate-y-0 gap-0 overflow-hidden p-0 sm:max-w-lg" :show-close-button="false">
      <DialogHeader class="sr-only">
        <DialogTitle>命令面板</DialogTitle>
        <DialogDescription>搜索账号、跳转到后台页面</DialogDescription>
      </DialogHeader>
      <Command class="rounded-lg">
        <div class="flex h-12 items-center gap-2 border-b px-3">
          <Search class="size-4 shrink-0 opacity-50" />
          <ListboxFilter
            v-model="query"
            auto-focus
            placeholder="输入页面名、用户名、邮箱或 ID…"
            class="h-full w-full bg-transparent text-sm outline-hidden placeholder:text-muted-foreground"
          />
          <Loader2 v-if="searching" class="size-4 shrink-0 animate-spin opacity-50" />
        </div>
        <AdminCommandAutoHighlight :watch="[users, pages]" />
        <CommandList class="max-h-[min(60vh,420px)]">
          <div v-if="nothing" class="py-8 text-center text-sm text-muted-foreground">
            没有找到「{{ keyword }}」相关的页面或账号
          </div>

          <CommandGroup v-if="users.length" heading="账号">
            <CommandItem v-for="user in users" :key="user.id" :value="`user-${user.id}`" @select="openUser(user)">
              <User />
              <span class="truncate">@{{ user.username }}</span>
              <span class="truncate text-xs text-muted-foreground">{{ user.email ?? `ID ${user.id}` }}</span>
              <span class="ml-auto flex gap-1">
                <Badge v-if="user.role !== 'user'" variant="secondary">{{ roleLabel[user.role] }}</Badge>
                <Badge v-if="user.status !== 'active'" variant="outline">{{ statusLabel[user.status] ?? user.status }}</Badge>
              </span>
            </CommandItem>
          </CommandGroup>

          <CommandGroup v-if="pages.length" heading="页面">
            <CommandItem v-for="item in pages" :key="item.to" :value="`page-${item.to}`" @select="openPage(item)">
              <component :is="item.icon" />
              {{ item.label }}
              <span class="ml-auto text-xs text-muted-foreground">{{ item.group }}</span>
            </CommandItem>
          </CommandGroup>

          <template v-if="!keyword">
            <CommandSeparator />
            <CommandGroup heading="操作">
              <CommandItem value="action-theme" @select="toggleTheme">
                <Moon />切换{{ mode === 'dark' ? '浅色' : '深色' }}模式
              </CommandItem>
            </CommandGroup>
          </template>
        </CommandList>
        <div class="flex items-center gap-3 border-t px-3 py-2 text-xs text-muted-foreground">
          <span class="flex items-center gap-1"><kbd class="rounded border px-1 font-mono">↑↓</kbd>选择</span>
          <span class="flex items-center gap-1"><kbd class="rounded border px-1"><CornerDownLeft class="size-3" /></kbd>打开</span>
          <span class="flex items-center gap-1"><kbd class="rounded border px-1 font-mono">Esc</kbd>关闭</span>
        </div>
      </Command>
    </DialogContent>
  </Dialog>
</template>
