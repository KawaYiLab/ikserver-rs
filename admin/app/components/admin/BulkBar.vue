<script setup lang="ts">
import { X } from '@lucide/vue'
import { Button } from '@/components/ui/button'

/** 勾选了行之后从底部浮出的批量操作条；默认插槽放操作按钮。 */
defineProps<{ count: number, unit?: string }>()
defineEmits<{ clear: [] }>()
</script>

<template>
  <Transition
    enter-active-class="transition duration-150 ease-out" enter-from-class="translate-y-4 opacity-0"
    leave-active-class="transition duration-100 ease-in" leave-to-class="translate-y-4 opacity-0"
  >
    <div
      v-if="count > 0"
      class="fixed inset-x-4 bottom-4 z-40 mx-auto flex max-w-xl items-center gap-2 rounded-lg border bg-popover p-2 pl-4 text-popover-foreground shadow-lg"
      role="region" aria-label="批量操作"
    >
      <span class="text-sm">已选 <span class="font-medium tabular-nums">{{ count }}</span> {{ unit ?? '项' }}</span>
      <div class="flex flex-1 flex-wrap justify-end gap-2">
        <slot />
      </div>
      <Button variant="ghost" size="icon" class="size-8" aria-label="取消选择" @click="$emit('clear')">
        <X class="size-4" />
      </Button>
    </div>
  </Transition>
</template>
