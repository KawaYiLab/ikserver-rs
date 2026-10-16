<script setup lang="ts">
import { useColorMode } from '@vueuse/core'
import {
  Bot, ClipboardList, FileText, Flag, LayoutDashboard, LogOut, Menu, Moon, Palette, Search, ShieldCheck, Sun, Tags, Users,
} from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from '@/components/ui/sheet'
import { roleLabel } from '~/lib/format'

const { me, isAdmin, logout } = useSession()
const mode = useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
const route = useRoute()
const mobileOpen = ref(false)
watch(() => route.fullPath, () => { mobileOpen.value = false })

const nav = computed(() => [
  { to: '/', label: '仪表盘', icon: LayoutDashboard, show: true },
  { to: '/reports', label: '举报处理', icon: Flag, show: true },
  { to: '/users', label: '账号', icon: Users, show: true },
  { to: '/exam', label: '入站题库', icon: ClipboardList, show: true },
  { to: '/content', label: '分区与标签', icon: Tags, show: true },
  { to: '/cosmetics', label: '装扮', icon: Palette, show: true },
  { to: '/ai', label: 'AI 助手', icon: Bot, show: isAdmin.value },
  { to: '/search', label: '搜索索引', icon: Search, show: isAdmin.value },
  { to: '/audit', label: '审计日志', icon: FileText, show: isAdmin.value },
].filter(item => item.show))

function isActive(to: string) {
  return to === '/' ? route.path === '/' : route.path.startsWith(to)
}

function toggleTheme() {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
}
</script>

<template>
  <div class="min-h-dvh bg-background">
    <aside class="fixed inset-y-0 left-0 hidden w-56 flex-col border-r bg-sidebar text-sidebar-foreground md:flex">
      <div class="flex h-14 items-center gap-2 border-b px-4 font-semibold">
        <ShieldCheck class="size-5" />
        绳网管理后台
      </div>
      <nav class="flex-1 space-y-1 overflow-y-auto p-2">
        <NuxtLink
          v-for="item in nav" :key="item.to" :to="item.to"
          class="flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors hover:bg-accent"
          :class="isActive(item.to) ? 'bg-accent font-medium text-accent-foreground' : 'text-muted-foreground'"
        >
          <component :is="item.icon" class="size-4" />
          {{ item.label }}
        </NuxtLink>
      </nav>
    </aside>

    <div class="md:pl-56">
      <header class="sticky top-0 z-30 flex h-14 items-center gap-2 border-b bg-background/90 px-4 backdrop-blur">
        <Sheet v-model:open="mobileOpen">
          <SheetTrigger as-child>
            <Button variant="ghost" size="icon" class="md:hidden" aria-label="打开菜单">
              <Menu class="size-5" />
            </Button>
          </SheetTrigger>
          <SheetContent side="left" class="w-60 p-0">
            <SheetTitle class="flex h-14 items-center border-b px-4">绳网管理后台</SheetTitle>
            <nav class="space-y-1 p-2">
              <NuxtLink
                v-for="item in nav" :key="item.to" :to="item.to"
                class="flex items-center gap-2 rounded-md px-3 py-2 text-sm hover:bg-accent"
                :class="isActive(item.to) ? 'bg-accent font-medium' : 'text-muted-foreground'"
              >
                <component :is="item.icon" class="size-4" />
                {{ item.label }}
              </NuxtLink>
            </nav>
          </SheetContent>
        </Sheet>
        <div class="flex-1" />
        <span v-if="me" class="hidden text-sm text-muted-foreground sm:inline">
          {{ me.username }} · {{ roleLabel[me.role] }}
        </span>
        <Button variant="ghost" size="icon" :aria-label="mode === 'dark' ? '切换到浅色' : '切换到深色'" @click="toggleTheme">
          <Sun v-if="mode === 'dark'" class="size-4" />
          <Moon v-else class="size-4" />
        </Button>
        <Button variant="ghost" size="sm" @click="logout">
          <LogOut class="size-4" />
          退出
        </Button>
      </header>
      <main class="mx-auto w-full max-w-7xl p-4 md:p-6">
        <slot />
      </main>
    </div>
  </div>
</template>
