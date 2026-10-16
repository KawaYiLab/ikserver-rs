<script setup lang="ts">
import { useColorMode } from '@vueuse/core'
import { Toaster } from '@/components/ui/sonner'
import 'vue-sonner/style.css'

// 跟随系统深浅色；用户在顶栏切换后记在本机。
useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
installUnauthorizedRedirect()
</script>

<template>
  <NuxtLayout>
    <NuxtPage />
  </NuxtLayout>
  <Toaster rich-colors position="top-center" />
  <AdminConfirmHost />
</template>
