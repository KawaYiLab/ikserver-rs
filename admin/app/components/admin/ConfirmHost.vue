<script setup lang="ts">
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { pendingConfirm } from '~/composables/useConfirm'

const typed = ref('')
watch(pendingConfirm, () => {
  typed.value = ''
})

const blocked = computed(() => {
  const need = pendingConfirm.value?.typeToConfirm
  return !!need && typed.value.trim() !== need
})

function answer(ok: boolean) {
  const pending = pendingConfirm.value
  if (!pending) return
  pendingConfirm.value = null
  pending.resolve(ok)
}
</script>

<template>
  <AlertDialog :open="!!pendingConfirm" @update:open="(open) => { if (!open) answer(false) }">
    <AlertDialogContent v-if="pendingConfirm">
      <AlertDialogHeader>
        <AlertDialogTitle>{{ pendingConfirm.title }}</AlertDialogTitle>
        <AlertDialogDescription v-if="pendingConfirm.description" class="whitespace-pre-wrap">
          {{ pendingConfirm.description }}
        </AlertDialogDescription>
      </AlertDialogHeader>
      <div v-if="pendingConfirm.typeToConfirm" class="space-y-2">
        <Label>请输入「{{ pendingConfirm.typeToConfirm }}」以继续</Label>
        <Input v-model="typed" :placeholder="pendingConfirm.typeToConfirm" @keydown.enter="!blocked && answer(true)" />
      </div>
      <AlertDialogFooter>
        <AlertDialogCancel @click="answer(false)">取消</AlertDialogCancel>
        <Button :variant="pendingConfirm.destructive ? 'destructive' : 'default'" :disabled="blocked" @click="answer(true)">
          {{ pendingConfirm.confirmLabel ?? '确认' }}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
