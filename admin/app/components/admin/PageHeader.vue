<script setup lang="ts">
defineProps<{ title: string, description?: string }>()
</script>

<template>
  <AdminReveal class="mb-8 flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
    <div class="min-w-0">
      <h1 class="text-[26px] leading-tight font-semibold tracking-[-0.02em]">
        {{ title }}
      </h1>
      <p v-if="description" class="mt-1.5 max-w-2xl text-sm text-muted-foreground">
        {{ description }}
      </p>
    </div>
    <div class="flex flex-wrap items-center gap-2">
      <slot />
    </div>
  </AdminReveal>
</template>
