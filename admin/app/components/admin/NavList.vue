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

function formatCount(count: number) {
  return count > 99 ? '99+' : String(count)
}
</script>

<template>
  <nav class="space-y-4 p-2">
    <div v-for="group in groups" :key="group.label">
      <div v-if="!props.collapsed" class="px-3 pb-1.5 text-[11px] font-medium tracking-wider text-sidebar-muted/80">
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
                'relative flex h-9 items-center gap-2.5 rounded-md px-3 text-sm transition-colors hover:bg-sidebar-accent hover:text-sidebar-foreground',
                current === item.to ? 'bg-sidebar-accent font-medium text-sidebar-foreground' : 'text-sidebar-muted',
                props.collapsed && 'justify-center px-0',
              )"
            >
              <span
                v-if="current === item.to"
                aria-hidden="true"
                :class="cn('absolute top-2 bottom-2 left-0 w-[3px] rounded-full bg-brand', props.collapsed && 'left-0.5')"
              />
              <component :is="item.icon" :class="cn('size-4 shrink-0', current === item.to && 'text-brand')" />
              <span v-if="!props.collapsed" class="flex-1 truncate">{{ item.label }}</span>
              <template v-if="item.badge">
                <span
                  v-if="badges[item.badge].count"
                  :class="cn(
                    'rounded-full bg-destructive px-1.5 text-[11px] leading-[18px] font-medium text-white tabular-nums',
                    props.collapsed && 'absolute -top-0.5 right-0.5 px-1 text-[10px] leading-4',
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
