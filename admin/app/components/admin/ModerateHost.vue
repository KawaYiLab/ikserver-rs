<script setup lang="ts">
import { toast } from 'vue-sonner'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { api, unwrap } from '~/lib/api'
import { pendingModerate } from '~/composables/useModerate'

const { failure } = useNotify()
const note = ref('')
const notify = ref(true)
const typed = ref('')
const submitting = ref(false)

const titles = { hide: '隐藏', unhide: '恢复可见', delete: '删除' } as const

watch(pendingModerate, (value) => {
  if (!value) return
  note.value = ''
  notify.value = value.action !== 'unhide'
  typed.value = ''
})

const request = computed(() => pendingModerate.value)
const isDelete = computed(() => request.value?.action === 'delete')
const blocked = computed(() => isDelete.value && typed.value.trim() !== '确认')

function close(result?: Awaited<ReturnType<typeof submitCall>>) {
  const pending = pendingModerate.value
  if (!pending) return
  pendingModerate.value = null
  pending.resolve(result)
}

function submitCall() {
  const r = request.value
  if (!r) return Promise.resolve(undefined)
  return unwrap(api.POST('/v1/admin/moderation/actions', {
    body: {
      targetType: r.targetType,
      ids: r.ids,
      action: r.action,
      note: note.value.trim() || null,
      notifyAuthor: r.action !== 'unhide' && notify.value,
    },
  }))
}

async function submit() {
  if (blocked.value || submitting.value) return
  submitting.value = true
  try {
    const result = await submitCall()
    if (result) {
      const parts = [`已${titles[result.action]} ${result.affected} 条`]
      if (result.unchanged.length) parts.push(`${result.unchanged.length} 条无需改动`)
      if (result.pendingReports.length) {
        toast.warning(`${result.pendingReports.length} 条有待处理的举报，已跳过`, {
          description: '请到「举报处理」里结案。',
          action: { label: '去处理', onClick: () => navigateTo('/reports') },
        })
      }
      toast.success(parts.join('，'))
    }
    close(result)
  }
  catch (error) {
    failure(error)
  }
  finally {
    submitting.value = false
  }
}
</script>

<template>
  <Dialog :open="!!request" @update:open="(open) => { if (!open) close() }">
    <DialogContent v-if="request">
      <DialogHeader>
        <DialogTitle>{{ titles[request.action] }}{{ request.label }}</DialogTitle>
        <DialogDescription>
          <template v-if="request.action === 'hide'">隐藏后只有作者本人能看到，随时可以恢复。</template>
          <template v-else-if="request.action === 'unhide'">恢复后重新公开。</template>
          <template v-else>删除不可恢复{{ request.targetType === 'comment' ? '，回复会一并删除' : '，帖子下的评论会一并删除' }}。</template>
          有待处理举报的会被跳过。
        </DialogDescription>
      </DialogHeader>
      <div class="space-y-4">
        <label class="grid gap-2">
          <span class="text-sm leading-none font-medium">处理备注（选填，写入审核日志）</span>
          <Textarea v-model="note" maxlength="500" rows="2" />
        </label>
        <label v-if="request.action !== 'unhide'" class="flex items-center gap-2 text-sm">
          <Checkbox v-model="notify" />
          给作者发系统通知（附上备注）
        </label>
        <div v-if="isDelete" class="space-y-2">
          <Label>这是不可恢复的操作，请输入「确认」</Label>
          <Input v-model="typed" placeholder="确认" @keydown.enter="submit" />
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" @click="close()">取消</Button>
        <Button :variant="request.action === 'unhide' ? 'default' : 'destructive'" :disabled="blocked || submitting" @click="submit">
          {{ titles[request.action] }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
