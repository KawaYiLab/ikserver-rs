<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { cn } from '@/lib/utils'
import { formatNumber } from '~/lib/format'

/**
 * 单系列按天柱状图（工作台趋势）。单系列不需要图例，标题由外层给出。
 * 每根柱子是悬停 / 键盘焦点的目标（命中区是整列高度）；最大值与今天直接标数，其余放在提示里；
 * 「数据表」切换给出不依赖悬停的完整数值。最后一天是今天，还没过完，提示里注明。
 */
const props = defineProps<{ days: string[], values: number[], label: string }>()

const HEIGHT = 220
const PAD = { top: 20, right: 8, bottom: 26, left: 36 }

const root = ref<HTMLElement | null>(null)
const { width } = useElementSize(root)
const showTable = ref(false)
const active = ref<number | null>(null)

const w = computed(() => Math.max(width.value, 240))
const plotW = computed(() => w.value - PAD.left - PAD.right)
const plotH = HEIGHT - PAD.top - PAD.bottom

/** 纵轴上限取「好看」的整数，刻度 4 格。 */
const ticks = computed(() => {
  const max = Math.max(...props.values, 0)
  if (max <= 4) return [0, 1, 2, 3, 4]
  // magnitude ≥ 1，步长取 1、2、5 的倍数，刻度都是整数。
  const raw = max / 4
  const magnitude = 10 ** Math.floor(Math.log10(raw))
  const step = [1, 2, 5, 10].map(m => m * magnitude).find(s => s >= raw) ?? raw
  return [0, 1, 2, 3, 4].map(i => Math.round(i * step))
})
const yMax = computed(() => ticks.value[ticks.value.length - 1] || 1)
const y = (v: number) => PAD.top + plotH - (v / yMax.value) * plotH

const band = computed(() => plotW.value / Math.max(props.values.length, 1))
const barW = computed(() => Math.max(2, Math.min(24, band.value - 2)))
const x = (i: number) => PAD.left + band.value * i + (band.value - barW.value) / 2

/** 顶端 4px 圆角、底端贴基线的柱子。 */
function barPath(i: number) {
  const v = props.values[i] ?? 0
  if (v <= 0) return ''
  const left = x(i)
  const top = y(v)
  const bottom = y(0)
  const r = Math.min(4, barW.value / 2, bottom - top)
  const right = left + barW.value
  return `M${left},${bottom}V${top + r}Q${left},${top} ${left + r},${top}H${right - r}Q${right},${top} ${right},${top + r}V${bottom}Z`
}

const maxIndex = computed(() => {
  let best = -1
  props.values.forEach((v, i) => { if (v > 0 && (best < 0 || v > (props.values[best] ?? 0))) best = i })
  return best
})
const lastIndex = computed(() => props.values.length - 1)
const labelled = computed(() => new Set([maxIndex.value, lastIndex.value].filter(i => i >= 0)))

/** 30 天时横轴每 5 天标一次（总带上今天）。 */
const xLabelEvery = computed(() => (props.values.length > 10 ? 5 : 1))
function showXLabel(i: number) {
  return i === lastIndex.value || (lastIndex.value - i) % xLabelEvery.value === 0
}
function shortDay(day?: string) {
  return day ? day.slice(5).replace('-', '/') : ''
}

const tooltip = computed(() => {
  const i = active.value
  if (i == null) return null
  const left = Math.min(Math.max(x(i) + barW.value / 2, 70), w.value - 70)
  return {
    left,
    value: formatNumber(props.values[i] ?? 0),
    day: `${props.days[i] ?? ''}${i === lastIndex.value ? '（今天，截至现在）' : ''}`,
  }
})
</script>

<template>
  <div>
    <div ref="root" class="relative" @pointerleave="active = null">
      <svg v-if="!showTable" :width="w" :height="HEIGHT" role="img" :aria-label="`${label}，最近 ${values.length} 天`" class="block overflow-visible">
        <!-- 网格与纵轴刻度：发丝线，退后 -->
        <g>
          <template v-for="t in ticks" :key="t">
            <line :x1="PAD.left" :x2="w - PAD.right" :y1="y(t)" :y2="y(t)" class="stroke-border" stroke-width="1" />
            <text :x="PAD.left - 6" :y="y(t)" dy="0.32em" text-anchor="end" class="fill-muted-foreground text-[10px] tabular-nums">{{ formatNumber(t) }}</text>
          </template>
        </g>
        <!-- 柱子 -->
        <g>
          <path
            v-for="(_, i) in values" :key="i" :d="barPath(i)"
            :class="cn(
              'transition-opacity duration-150',
              i === lastIndex ? 'fill-brand stroke-brand-strong' : 'fill-chart-1',
              active != null && active !== i && 'opacity-50',
            )"
            stroke-width="1"
          />
        </g>
        <!-- 直接标注：最大值与今天 -->
        <g>
          <template v-for="i in labelled" :key="i">
            <text
              v-if="(values[i] ?? 0) > 0" :x="x(i) + barW / 2" :y="y(values[i] ?? 0) - 6" text-anchor="middle"
              class="fill-foreground text-[11px] font-medium tabular-nums"
            >{{ formatNumber(values[i]) }}</text>
          </template>
        </g>
        <!-- 横轴 -->
        <g>
          <template v-for="(day, i) in days" :key="day">
            <text
              v-if="showXLabel(i)" :x="PAD.left + band * i + band / 2" :y="HEIGHT - 8" text-anchor="middle"
              class="fill-muted-foreground text-[10px] tabular-nums"
            >{{ i === lastIndex ? '今天' : shortDay(day) }}</text>
          </template>
        </g>
        <!-- 命中区：整列高度，比柱子大 -->
        <g>
          <rect
            v-for="(_, i) in values" :key="i"
            :x="PAD.left + band * i" :y="PAD.top" :width="band" :height="plotH" fill="transparent"
            tabindex="0" class="outline-none focus-visible:fill-foreground/5"
            :aria-label="`${days[i]}：${values[i]}`"
            @pointerenter="active = i" @focus="active = i" @blur="active = null"
          />
        </g>
      </svg>
      <div
        v-if="tooltip && !showTable"
        class="pointer-events-none absolute top-0 -translate-x-1/2 rounded-md border bg-popover px-2.5 py-1.5 text-xs shadow-md"
        :style="{ left: `${tooltip.left}px` }"
      >
        <div class="text-sm font-semibold tabular-nums text-popover-foreground">{{ tooltip.value }}</div>
        <div class="flex items-center gap-1.5 text-muted-foreground">
          <span class="inline-block size-2 rounded-sm bg-chart-1" />{{ label }} · {{ tooltip.day }}
        </div>
      </div>

      <div v-if="showTable" class="max-h-[220px] overflow-y-auto rounded-md border">
        <table class="w-full text-sm">
          <thead class="sticky top-0 bg-muted text-muted-foreground">
            <tr><th class="px-3 py-1.5 text-left font-medium">业务日</th><th class="px-3 py-1.5 text-right font-medium">{{ label }}</th></tr>
          </thead>
          <tbody>
            <tr v-for="(day, i) in days" :key="day" class="border-t">
              <td class="px-3 py-1 tabular-nums">{{ day }}<span v-if="i === lastIndex" class="text-muted-foreground">（今天）</span></td>
              <td class="px-3 py-1 text-right tabular-nums">{{ formatNumber(values[i]) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
    <div class="mt-2 flex justify-end">
      <button type="button" class="text-xs text-muted-foreground underline-offset-2 hover:underline" @click="showTable = !showTable">
        {{ showTable ? '看图表' : '看数据表' }}
      </button>
    </div>
  </div>
</template>
