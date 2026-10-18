<script setup lang="ts">
import { MoreHorizontal } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { RowAction } from '~/lib/actions'

/** 表格行末的「⋯」操作菜单。 */
const props = defineProps<{ actions: RowAction[], label?: string }>()

const visible = computed(() => props.actions.filter(action => !action.hidden))
</script>

<template>
  <DropdownMenu v-if="visible.length">
    <DropdownMenuTrigger as-child>
      <Button variant="ghost" size="icon" class="size-8" :aria-label="label ?? '更多操作'">
        <MoreHorizontal class="size-4" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="min-w-40">
      <template v-for="(action, index) in visible" :key="action.label">
        <DropdownMenuSeparator v-if="action.destructive && index > 0 && !visible[index - 1]?.destructive" />
        <DropdownMenuItem
          v-if="action.href" as-child
          :disabled="!!action.disabledReason"
        >
          <a :href="action.href" target="_blank" rel="noopener">
            <component :is="action.icon" v-if="action.icon" />{{ action.label }}
          </a>
        </DropdownMenuItem>
        <DropdownMenuItem
          v-else
          :variant="action.destructive ? 'destructive' : 'default'"
          :disabled="!!action.disabledReason"
          class="flex-wrap"
          @select="action.onSelect?.()"
        >
          <component :is="action.icon" v-if="action.icon" />{{ action.label }}
          <span v-if="action.disabledReason" class="w-full pl-6 text-xs text-muted-foreground">{{ action.disabledReason }}</span>
        </DropdownMenuItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
