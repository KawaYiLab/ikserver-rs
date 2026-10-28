<script setup lang="ts">
import { useColorMode } from '@vueuse/core'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import 'vue-sonner/style.css'

// 跟随系统深浅色；用户在顶栏切换后记在本机。
useColorMode({ selector: 'html', attribute: 'class', storageKey: 'ik-admin-theme' })
installUnauthorizedRedirect()
</script>

<template>
  <MotionConfig reduced-motion="user">
    <TooltipProvider :delay-duration="300">
      <NuxtLayout>
        <NuxtPage />
      </NuxtLayout>
    </TooltipProvider>
  </MotionConfig>
  <Toaster rich-colors position="top-center" />
  <AdminConfirmHost />
  <AdminModerateHost />
</template>
