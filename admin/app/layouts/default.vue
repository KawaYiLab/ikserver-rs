<script setup lang="ts">
import { useColorMode, useStorage } from '@vueuse/core'
import { LogOut, Menu, Moon, PanelLeftClose, PanelLeftOpen, Search, ShieldCheck, Sun } from '@lucide/vue'
import { Button } from '@/components/ui/button'
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
useHead({ title: () => (title.value ? `${title.value} · 绳网管理后台` : '绳网管理后台') })

function toggleTheme() {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
}
</script>

<template>
  <div class="min-h-dvh bg-background">
    <aside
      :class="cn(
        'fixed inset-y-0 left-0 z-40 hidden flex-col border-r bg-sidebar text-sidebar-foreground transition-[width] md:flex',
        collapsed ? 'w-14' : 'w-56',
      )"
    >
      <div :class="cn('flex h-14 shrink-0 items-center gap-2 border-b font-semibold', collapsed ? 'justify-center' : 'px-4')">
        <ShieldCheck class="size-5 shrink-0" />
        <span v-if="!collapsed" class="truncate">绳网管理后台</span>
      </div>
      <div class="flex-1 overflow-y-auto">
        <AdminNavList :collapsed="collapsed" />
      </div>
      <div class="border-t p-2">
        <Button
          variant="ghost" size="sm" :class="cn('w-full text-muted-foreground', collapsed ? 'justify-center px-0' : 'justify-start')"
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
          <SheetContent side="left" class="w-60 gap-0 p-0">
            <SheetTitle class="flex h-14 items-center gap-2 border-b px-4">
              <ShieldCheck class="size-5" />绳网管理后台
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
        <span v-if="me" class="hidden text-sm text-muted-foreground lg:inline">
          {{ me.username }} · {{ roleLabel[me.role] }}
        </span>
        <Button variant="ghost" size="icon" :aria-label="mode === 'dark' ? '切换到浅色' : '切换到深色'" @click="toggleTheme">
          <Sun v-if="mode === 'dark'" class="size-4" />
          <Moon v-else class="size-4" />
        </Button>
        <Button variant="ghost" size="sm" @click="logout">
          <LogOut class="size-4" />
          <span class="hidden sm:inline">退出</span>
        </Button>
      </header>
      <main class="mx-auto w-full max-w-7xl p-4 md:p-6">
        <slot />
      </main>
    </div>
    <AdminCommandPalette v-model:open="paletteOpen" />
  </div>
</template>
