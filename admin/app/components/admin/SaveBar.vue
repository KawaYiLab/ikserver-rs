<script setup lang="ts">
import { Button } from '@/components/ui/button'

/** 设置类页面底部的「未保存」浮条。 */
defineProps<{ count: number, saving?: boolean }>()
defineEmits<{ save: [], discard: [] }>()
</script>

<template>
  <Transition
    enter-active-class="transition duration-150 ease-out" enter-from-class="translate-y-2 opacity-0"
    leave-active-class="transition duration-100 ease-in" leave-to-class="translate-y-2 opacity-0"
  >
    <div v-if="count" class="fixed inset-x-0 bottom-4 z-30 flex justify-center px-4">
      <div class="flex w-full max-w-xl items-center gap-3 rounded-xl border bg-popover px-4 py-3 shadow-lg">
        <span class="size-2 shrink-0 rounded-full bg-brand-strong" />
        <span class="flex-1 text-sm">{{ count }} 项修改未保存</span>
        <Button variant="ghost" size="sm" :disabled="saving" @click="$emit('discard')">
          放弃
        </Button>
        <Button size="sm" :disabled="saving" @click="$emit('save')">
          {{ saving ? '保存中…' : '保存' }}
        </Button>
      </div>
    </div>
  </Transition>
</template>
