<script setup lang="ts">
import { RotateCcw } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { SettingField, SettingValue } from '~/lib/settings'
import { sameValue } from '~/lib/settings'

/** 设置页的一行：名称与说明在左，控件在右；和部署配置的缺省值不同时标出来，可以一键恢复。 */
const props = defineProps<{
  field: SettingField
  modelValue: SettingValue
  defaultValue: SettingValue
  /** 和保存前的值不同（还没保存）。 */
  dirty?: boolean
  error?: string
  disabled?: boolean
}>()
const emit = defineEmits<{ 'update:modelValue': [value: SettingValue] }>()

const id = computed(() => `setting-${props.field.path.replace('.', '-')}`)
const changed = computed(() => !sameValue(props.modelValue, props.defaultValue))
/** 文本类控件占满一行，名称放在上面。 */
const wide = computed(() => props.field.kind === 'text' || props.field.kind === 'textarea')

function display(value: SettingValue) {
  if (typeof value === 'boolean') return value ? '开' : '关'
  if (value === null || value === '') return '空'
  if (props.field.kind === 'select') return props.field.options?.find(o => o.value === value)?.label ?? String(value)
  if (props.field.kind === 'datetime') return new Date(value as string).toLocaleString('zh-CN')
  if (props.field.kind === 'text' || props.field.kind === 'textarea') return `「${value}」`
  return `${value}${props.field.unit ? ` ${props.field.unit}` : ''}`
}

// datetime-local 用本地时间、精确到分钟；接口用 ISO 时间，空为 null。
function toLocalInput(value: SettingValue) {
  if (!value) return ''
  const date = new Date(value as string)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}
function onDatetime(raw: string | number) {
  emit('update:modelValue', raw ? new Date(String(raw)).toISOString() : null)
}

const length = computed(() => (typeof props.modelValue === 'string' ? [...props.modelValue].length : 0))

function onNumber(raw: string | number) {
  const value = Number(raw)
  emit('update:modelValue', raw === '' || Number.isNaN(value) ? raw as unknown as number : value)
}
</script>

<template>
  <div :class="['flex flex-col gap-3 py-4', !wide && 'sm:flex-row sm:items-center sm:justify-between sm:gap-6']">
    <div class="relative min-w-0 flex-1">
      <Tooltip v-if="changed && wide">
        <TooltipTrigger as-child>
          <Button
            variant="ghost" size="icon-sm" class="absolute top-0 right-0 text-muted-foreground" :disabled="disabled"
            :aria-label="`${field.label}恢复默认`" @click="emit('update:modelValue', defaultValue)"
          >
            <RotateCcw />
          </Button>
        </TooltipTrigger>
        <TooltipContent>恢复默认</TooltipContent>
      </Tooltip>
      <label :for="id" class="flex items-center gap-2 text-sm font-medium">
        {{ field.label }}
        <span v-if="dirty" class="size-1.5 rounded-full bg-brand-strong" aria-label="未保存" />
      </label>
      <p v-if="field.help" class="mt-0.5 text-xs text-muted-foreground">
        {{ field.help }}
      </p>
      <p v-if="changed" class="mt-0.5 text-xs text-muted-foreground">
        默认 {{ display(defaultValue) }}
      </p>
      <p v-if="error" class="mt-1 text-xs text-destructive" role="alert">
        {{ error }}
      </p>
    </div>
    <div :class="['flex items-center gap-1.5', wide ? 'w-full' : 'shrink-0']">
      <Tooltip v-if="changed && !wide">
        <TooltipTrigger as-child>
          <Button
            variant="ghost" size="icon-sm" class="text-muted-foreground" :disabled="disabled"
            :aria-label="`${field.label}恢复默认`" @click="emit('update:modelValue', defaultValue)"
          >
            <RotateCcw />
          </Button>
        </TooltipTrigger>
        <TooltipContent>恢复默认</TooltipContent>
      </Tooltip>
      <Switch
        v-if="field.kind === 'switch'" :id="id" :model-value="modelValue as boolean" :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event)"
      />
      <div v-else-if="field.kind === 'textarea'" class="relative w-full">
        <Textarea
          :id="id" :model-value="modelValue as string" :placeholder="field.placeholder" :disabled="disabled" rows="2"
          :aria-invalid="error ? true : undefined" class="resize-y pb-6"
          @update:model-value="emit('update:modelValue', String($event))"
        />
        <span v-if="field.maxLength" class="pointer-events-none absolute right-3 bottom-1.5 text-[11px] text-muted-foreground tabular-nums">
          {{ length }}/{{ field.maxLength }}
        </span>
      </div>
      <Input
        v-else-if="field.kind === 'text'" :id="id" :model-value="modelValue as string" :placeholder="field.placeholder"
        :maxlength="field.maxLength" :disabled="disabled" :aria-invalid="error ? true : undefined"
        @update:model-value="emit('update:modelValue', String($event))"
      />
      <Select
        v-else-if="field.kind === 'select'" :model-value="modelValue as string" :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event as string)"
      >
        <SelectTrigger :id="id" class="w-full sm:w-40">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in field.options" :key="option.value" :value="option.value">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Input
        v-else-if="field.kind === 'datetime'" :id="id" type="datetime-local" class="w-full sm:w-56"
        :model-value="toLocalInput(modelValue)" :disabled="disabled" :aria-invalid="error ? true : undefined"
        @update:model-value="onDatetime"
      />
      <div v-else class="relative w-full sm:w-40">
        <Input
          :id="id" type="number" inputmode="decimal" :min="field.min" :max="field.max" :step="field.step ?? 1"
          :model-value="modelValue as number" :disabled="disabled" :aria-invalid="error ? true : undefined"
          :class="['tabular-nums [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none', field.unit && 'pr-12']"
          @update:model-value="onNumber"
        />
        <span v-if="field.unit" class="pointer-events-none absolute inset-y-0 right-3 flex items-center text-xs text-muted-foreground">
          {{ field.unit }}
        </span>
      </div>
    </div>
  </div>
</template>
