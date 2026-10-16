<script setup lang="ts">
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'

defineProps<{ title: string, value: string | number, hint?: string, tone?: 'default' | 'warning' | 'danger' }>()
</script>

<template>
  <Card>
    <CardHeader class="pb-2">
      <CardTitle class="text-sm font-medium text-muted-foreground">
        {{ title }}
      </CardTitle>
    </CardHeader>
    <CardContent>
      <div
        class="text-2xl font-semibold tabular-nums"
        :class="{ 'text-warning': tone === 'warning', 'text-destructive': tone === 'danger' }"
      >
        {{ value }}
      </div>
      <p v-if="hint" class="mt-1 text-xs text-muted-foreground">
        {{ hint }}
      </p>
    </CardContent>
  </Card>
</template>
