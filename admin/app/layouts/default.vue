<script setup lang="ts">
import { useColorMode, useStorage } from '@vueuse/core'
import { ExternalLink, LogOut, Menu, Moon, PanelLeftClose, PanelLeftOpen, Search, ShieldCheck, Sun } from '@lucide/vue'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { roleLabel } from '~/lib/format'
import { navItemFor } from '~/lib/nav'

const { me, logout } = useSession()
const mode = useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
const route = useRoute()
const mobileOpen = ref(false)
watch(() => route.fullPath, () => { mobileOpen.value = false })

// 折叠状态只是本机偏好；存储不可用（隐私模式等）时 useStorage 退回内存值。
const collapsed = useStorage('ik-admin-sidebar-collapsed', false)
const paletteOpen = ref(false)
const isMac = import.meta.client && /Mac|iPhone|iPad/.test(navigator.platform)

const title = computed(() => navItemFor(route.path)?.label ?? '')
const siteUrl = useRuntimeConfig().public.siteUrl
const initial = computed(() => me.value?.username.slice(0, 1).toUpperCase() ?? '')
useHead({ title: () => (title.value ? `${title.value} · 绳网管理后台` : '绳网管理后台') })

function toggleTheme() {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
}
</script>

<template>
  <div class="min-h-dvh bg-background">
    <aside
      :class="cn(
        'fixed inset-y-0 left-0 z-40 hidden flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground transition-[width] md:flex',
        collapsed ? 'w-14' : 'w-56',
      )"
    >
      <div :class="cn('flex h-14 shrink-0 items-center gap-2.5 font-semibold', collapsed ? 'justify-center' : 'px-4')">
        <span class="grid size-7 shrink-0 place-items-center rounded-md bg-brand text-brand-foreground">
          <ShieldCheck class="size-4" />
        </span>
        <span v-if="!collapsed" class="truncate">绳网管理后台</span>
      </div>
      <div class="flex-1 overflow-y-auto">
        <AdminNavList :collapsed="collapsed" />
      </div>
      <div class="border-t border-sidebar-border p-2">
        <Button
          variant="ghost" size="sm" :class="cn('w-full text-sidebar-muted hover:bg-sidebar-accent hover:text-sidebar-foreground', collapsed ? 'justify-center px-0' : 'justify-start')"
          :aria-label="collapsed ? '展开侧边栏' : '收起侧边栏'" @click="collapsed = !collapsed"
        >
          <PanelLeftOpen v-if="collapsed" class="size-4" />
          <template v-else>
            <PanelLeftClose class="size-4" />收起
          </template>
        </Button>
      </div>
    </aside>

    <div :class="cn('transition-[padding]', collapsed ? 'md:pl-14' : 'md:pl-56')">
      <header class="sticky top-0 z-30 flex h-14 items-center gap-2 border-b bg-background/90 px-4 backdrop-blur">
        <Sheet v-model:open="mobileOpen">
          <SheetTrigger as-child>
            <Button variant="ghost" size="icon" class="md:hidden" aria-label="打开菜单">
              <Menu class="size-5" />
            </Button>
          </SheetTrigger>
          <SheetContent side="left" class="w-60 gap-0 border-sidebar-border bg-sidebar p-0 text-sidebar-foreground">
            <SheetTitle class="flex h-14 items-center gap-2.5 px-4 text-sidebar-foreground">
              <span class="grid size-7 place-items-center rounded-md bg-brand text-brand-foreground">
                <ShieldCheck class="size-4" />
              </span>绳网管理后台
            </SheetTitle>
            <div class="overflow-y-auto">
              <AdminNavList />
            </div>
          </SheetContent>
        </Sheet>
        <span class="truncate text-sm font-medium md:hidden">{{ title }}</span>
        <button
          type="button"
          class="hidden h-9 w-full max-w-sm items-center gap-2 rounded-md border bg-muted/40 px-3 text-sm text-muted-foreground transition-colors hover:bg-muted sm:flex"
          @click="paletteOpen = true"
        >
          <Search class="size-4" />
          <span class="flex-1 text-left">搜索账号、跳转页面…</span>
          <kbd class="rounded border bg-background px-1.5 font-mono text-[10px]">{{ isMac ? '⌘' : 'Ctrl' }} K</kbd>
        </button>
        <div class="flex-1" />
        <Button variant="ghost" size="icon" class="sm:hidden" aria-label="搜索" @click="paletteOpen = true">
          <Search class="size-4" />
        </Button>
        <Button variant="ghost" size="icon" :aria-label="mode === 'dark' ? '切换到浅色' : '切换到深色'" @click="toggleTheme">
          <Sun v-if="mode === 'dark'" class="size-4" />
          <Moon v-else class="size-4" />
        </Button>
        <DropdownMenu v-if="me">
          <DropdownMenuTrigger as-child>
            <Button variant="ghost" class="h-9 gap-2 px-1.5 lg:pr-3" aria-label="账号菜单">
              <Avatar class="size-7">
                <AvatarFallback class="bg-brand text-xs font-semibold text-brand-foreground">{{ initial }}</AvatarFallback>
              </Avatar>
              <span class="hidden text-left leading-tight lg:block">
                <span class="block text-sm font-medium">{{ me.username }}</span>
                <span class="block text-[11px] font-normal text-muted-foreground">{{ roleLabel[me.role] }}</span>
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-48">
            <DropdownMenuLabel class="font-normal">
              <div class="text-sm font-medium">{{ me.username }}</div>
              <div class="text-xs text-muted-foreground">{{ roleLabel[me.role] }}</div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem as-child>
              <a :href="siteUrl" target="_blank" rel="noopener"><ExternalLink class="size-4" />打开主站</a>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" @select="logout">
              <LogOut class="size-4" />退出登录
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </header>
      <main class="mx-auto w-full max-w-7xl p-4 md:p-6">
        <slot />
      </main>
    </div>
    <AdminCommandPalette v-model:open="paletteOpen" />
  </div>
</template>
