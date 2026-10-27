<script setup lang="ts">
import { Textarea } from '@/components/ui/textarea'

/** 每行一项的列表输入：去掉首尾空白与空行、重复项；`lowercase` 时统一转小写。 */
const props = defineProps<{
  modelValue: string[]
  placeholder?: string
  lowercase?: boolean
  disabled?: boolean
  invalid?: boolean
  rows?: number
}>()
const emit = defineEmits<{ 'update:modelValue': [value: string[]] }>()

function parse(text: string) {
  const lines = text.split('\n').map(line => (props.lowercase ? line.trim().toLowerCase() : line.trim())).filter(Boolean)
  return [...new Set(lines)]
}

const text = ref(props.modelValue.join('\n'))
// 外面换了值（放弃修改、恢复缺省）才重写文本，正在输入时不打断。
watch(() => props.modelValue, (next) => {
  if (JSON.stringify(parse(text.value)) !== JSON.stringify(next)) text.value = next.join('\n')
})

function onInput(raw: string | number) {
  text.value = String(raw)
  emit('update:modelValue', parse(text.value))
}
</script>

<template>
  <div class="relative">
    <Textarea
      :model-value="text" :placeholder="placeholder" :disabled="disabled" :rows="rows ?? 4"
      :aria-invalid="invalid ? true : undefined" class="max-h-72 resize-y pb-6 font-mono text-[13px]"
      @update:model-value="onInput"
    />
    <span class="pointer-events-none absolute right-3 bottom-1.5 text-[11px] text-muted-foreground tabular-nums">
      {{ modelValue.length }} 项
    </span>
  </div>
</template>
