<script setup lang="ts">
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { navItemFor, visibleNav } from '~/lib/nav'

/** 侧边栏导航（桌面与移动端共用）。`collapsed` 时只显示图标，悬停显示名称。 */
const props = defineProps<{ collapsed?: boolean }>()

const { isAdmin } = useSession()
const { badges } = useBadges()
const route = useRoute()
const groups = computed(() => visibleNav(isAdmin.value))
const current = computed(() => navItemFor(route.path)?.to)

// 桌面侧栏与移动端抽屉各有一份，选中胶囊的 layoutId 要分开，免得互相飞。
const layoutId = useId()

function formatCount(count: number) {
  return count > 99 ? '99+' : String(count)
}
</script>

<template>
  <nav class="space-y-5 px-3 py-2">
    <div v-for="group in groups" :key="group.label">
      <div v-if="!props.collapsed" class="px-2.5 pb-1.5 text-[11px] font-medium text-sidebar-muted/75">
        {{ group.label }}
      </div>
      <div v-else class="mx-auto mb-2 h-px w-6 bg-sidebar-border first:hidden" />
      <div class="space-y-0.5">
        <Tooltip v-for="item in group.items" :key="item.to" :disabled="!props.collapsed">
          <TooltipTrigger as-child>
            <NuxtLink
              :to="item.to"
              :aria-current="current === item.to ? 'page' : undefined"
              :class="cn(
                'group/nav relative isolate flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[13px] transition-colors',
                current === item.to ? 'font-medium text-sidebar-foreground' : 'text-sidebar-muted hover:bg-sidebar-accent hover:text-sidebar-foreground',
                props.collapsed && 'justify-center px-0',
              )"
            >
              <!-- 选中胶囊：切换页面时滑到新位置（只在切换时动一次）。 -->
              <Motion
                v-if="current === item.to"
                :layout-id="`nav-active-${layoutId}`"
                aria-hidden="true"
                class="absolute inset-0 -z-10 rounded-lg bg-sidebar-active shadow-[0_0_0_1px_var(--sidebar-border),0_1px_2px_oklch(0_0_0/6%)]"
                :transition="{ type: 'spring', stiffness: 520, damping: 40, mass: 0.8 }"
              />
              <component
                :is="item.icon"
                :class="cn('size-4 shrink-0 transition-colors', current === item.to ? 'text-sidebar-foreground' : 'text-sidebar-muted/80 group-hover/nav:text-sidebar-foreground')"
              />
              <span v-if="!props.collapsed" class="flex-1 truncate">{{ item.label }}</span>
              <span
                v-if="current === item.to && !props.collapsed && !item.badge"
                aria-hidden="true" class="size-1.5 rounded-full bg-brand-strong"
              />
              <template v-if="item.badge">
                <span
                  v-if="badges[item.badge].count"
                  :class="cn(
                    'rounded-md bg-destructive/10 px-1.5 text-[11px] leading-[18px] font-semibold text-destructive tabular-nums',
                    props.collapsed && 'absolute -top-1 right-0 bg-destructive px-1 text-[10px] leading-4 text-white',
                  )"
                  :aria-label="`${badges[item.badge].count} 项待处理`"
                >{{ formatCount(badges[item.badge].count ?? 0) }}</span>
                <span
                  v-else-if="badges[item.badge].alert"
                  :class="cn('size-2 rounded-full bg-warning', props.collapsed && 'absolute top-1 right-2')"
                  aria-label="需要注意"
                />
              </template>
            </NuxtLink>
          </TooltipTrigger>
          <TooltipContent side="right">
            {{ item.label }}
            <template v-if="item.badge && badges[item.badge].count"> · {{ badges[item.badge].count }}</template>
          </TooltipContent>
        </Tooltip>
      </div>
    </div>
  </nav>
</template>
