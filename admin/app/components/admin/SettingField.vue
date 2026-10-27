<script setup lang="ts">
import { RotateCcw } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
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

function display(value: SettingValue) {
  if (typeof value === 'boolean') return value ? '开' : '关'
  return `${value}${props.field.unit ? ` ${props.field.unit}` : ''}`
}

function onNumber(raw: string | number) {
  const value = Number(raw)
  emit('update:modelValue', raw === '' || Number.isNaN(value) ? raw as unknown as number : value)
}
</script>

<template>
  <div class="flex flex-col gap-3 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
    <div class="min-w-0 flex-1">
      <label :for="id" class="flex items-center gap-2 text-sm font-medium">
        {{ field.label }}
        <span v-if="dirty" class="size-1.5 rounded-full bg-brand-strong" aria-label="未保存" />
      </label>
      <p v-if="field.help" class="mt-0.5 text-xs text-muted-foreground">
        {{ field.help }}
      </p>
      <p v-if="changed" class="mt-0.5 text-xs text-muted-foreground">
        部署配置为 {{ display(defaultValue) }}
      </p>
      <p v-if="error" class="mt-1 text-xs text-destructive" role="alert">
        {{ error }}
      </p>
    </div>
    <div class="flex shrink-0 items-center gap-1.5">
      <Tooltip v-if="changed">
        <TooltipTrigger as-child>
          <Button
            variant="ghost" size="icon-sm" class="text-muted-foreground" :disabled="disabled"
            :aria-label="`${field.label}恢复为部署配置`" @click="emit('update:modelValue', defaultValue)"
          >
            <RotateCcw />
          </Button>
        </TooltipTrigger>
        <TooltipContent>恢复为部署配置</TooltipContent>
      </Tooltip>
      <Switch
        v-if="field.kind === 'switch'" :id="id" :model-value="modelValue as boolean" :disabled="disabled"
        @update:model-value="emit('update:modelValue', $event)"
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
