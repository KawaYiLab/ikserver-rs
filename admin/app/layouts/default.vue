<script setup lang="ts">
import { useColorMode, useStorage } from '@vueuse/core'
import { ChevronRight, ExternalLink, LogOut, Menu, Moon, PanelLeftClose, PanelLeftOpen, Search, Sun } from '@lucide/vue'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { roleLabel } from '~/lib/format'
import { navGroups, navItemFor } from '~/lib/nav'

const { me, avatarUrl, logout } = useSession()
const mode = useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
const route = useRoute()
const mobileOpen = ref(false)
watch(() => route.fullPath, () => { mobileOpen.value = false })

// 折叠状态只是本机偏好；存储不可用（隐私模式等）时 useStorage 退回内存值。
const collapsed = useStorage('ik-admin-sidebar-collapsed', false)
const paletteOpen = ref(false)
const isMac = import.meta.client && /Mac|iPhone|iPad/.test(navigator.platform)

const title = computed(() => navItemFor(route.path)?.label ?? '')
const section = computed(() => navGroups.find(group => group.items.some(item => item.to === navItemFor(route.path)?.to))?.label ?? '')
const siteUrl = useRuntimeConfig().public.siteUrl
const initial = computed(() => me.value?.username.slice(0, 1).toUpperCase() ?? '')
useHead({ title: () => (title.value ? `${title.value} · 绳网管理后台` : '绳网管理后台') })

function toggleTheme() {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
}
</script>

<template>
  <div class="min-h-dvh bg-canvas">
    <aside
      :class="cn(
        'fixed inset-y-0 left-0 z-40 hidden flex-col bg-sidebar text-sidebar-foreground transition-[width] duration-200 ease-out md:flex',
        collapsed ? 'w-16' : 'w-60',
      )"
    >
      <div :class="cn('flex h-16 shrink-0 items-center gap-2.5', collapsed ? 'justify-center' : 'px-5')">
        <AdminBrandMark />
        <span v-if="!collapsed" class="min-w-0 leading-tight">
          <span class="block truncate text-sm font-semibold tracking-tight">绳网</span>
          <span class="block truncate text-[11px] text-sidebar-muted">管理后台</span>
        </span>
      </div>
      <div class="flex-1 overflow-y-auto [scrollbar-width:thin]">
        <AdminNavList :collapsed="collapsed" />
      </div>
      <div class="p-3">
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

    <div :class="cn('transition-[padding] duration-200 ease-out md:py-2 md:pr-2', collapsed ? 'md:pl-16' : 'md:pl-60')">
      <div class="min-h-dvh bg-background md:min-h-[calc(100dvh-1rem)] md:rounded-2xl md:shadow-[var(--shadow-panel)]">
        <header class="sticky top-0 z-30 flex h-14 items-center gap-2 border-b border-border/70 bg-background/80 px-4 backdrop-blur-md backdrop-saturate-150 md:top-2 md:rounded-t-2xl md:px-6">
          <Sheet v-model:open="mobileOpen">
            <SheetTrigger as-child>
              <Button variant="ghost" size="icon" class="-ml-2 md:hidden" aria-label="打开菜单">
                <Menu class="size-5" />
              </Button>
            </SheetTrigger>
            <SheetContent side="left" class="w-64 gap-0 border-sidebar-border bg-sidebar p-0 text-sidebar-foreground">
              <SheetTitle class="flex h-16 items-center gap-2.5 px-5 text-sidebar-foreground">
                <AdminBrandMark />绳网管理后台
              </SheetTitle>
              <div class="overflow-y-auto">
                <AdminNavList />
              </div>
            </SheetContent>
          </Sheet>
          <nav aria-label="位置" class="flex min-w-0 items-center gap-1.5 text-sm">
            <span v-if="section" class="hidden text-muted-foreground sm:inline">{{ section }}</span>
            <ChevronRight v-if="section" class="hidden size-3.5 text-muted-foreground/60 sm:inline" />
            <span class="truncate font-medium">{{ title }}</span>
          </nav>
          <div class="flex-1" />
          <button
            type="button"
            class="hidden h-8 w-64 items-center gap-2 rounded-lg border bg-muted/50 px-2.5 text-[13px] text-muted-foreground transition-colors hover:border-foreground/15 hover:bg-muted sm:flex"
            @click="paletteOpen = true"
          >
            <Search class="size-3.5" />
            <span class="flex-1 text-left">搜索账号、跳转页面…</span>
            <kbd class="rounded border bg-background px-1.5 font-sans text-[10px] font-medium shadow-[0_1px_0_var(--border)]">{{ isMac ? '⌘' : 'Ctrl' }} K</kbd>
          </button>
          <Button variant="ghost" size="icon" class="sm:hidden" aria-label="搜索" @click="paletteOpen = true">
            <Search class="size-4" />
          </Button>
          <Button variant="ghost" size="icon" class="size-8" :aria-label="mode === 'dark' ? '切换到浅色' : '切换到深色'" @click="toggleTheme">
            <Sun v-if="mode === 'dark'" class="size-4" />
            <Moon v-else class="size-4" />
          </Button>
          <DropdownMenu v-if="me">
            <DropdownMenuTrigger as-child>
              <Button variant="ghost" size="icon" class="size-8 rounded-full" aria-label="账号菜单">
                <Avatar class="size-7 ring-2 ring-background">
                  <AvatarImage v-if="avatarUrl" :src="avatarUrl" :alt="me.username" class="object-cover" />
                  <AvatarFallback class="bg-brand text-xs font-semibold text-brand-foreground">{{ initial }}</AvatarFallback>
                </Avatar>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-52">
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
        <main class="mx-auto w-full max-w-[84rem] px-4 py-6 md:px-8 md:py-8">
          <slot />
        </main>
      </div>
    </div>
    <AdminCommandPalette v-model:open="paletteOpen" />
  </div>
</template>
