<script setup lang="ts">
import { ShieldCheck } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ApiError } from '~/lib/api'

definePageMeta({ layout: false })

const { login, isStaff, logout } = useSession()
const route = useRoute()
const identifier = ref('')
const password = ref('')
const pending = ref(false)
const error = ref('')

async function submit() {
  error.value = ''
  pending.value = true
  try {
    await login(identifier.value.trim(), password.value)
    if (!isStaff.value) {
      error.value = '这个账号没有后台权限'
      await logout()
      return
    }
    const next = typeof route.query.next === 'string' && route.query.next.startsWith('/') ? route.query.next : '/'
    await navigateTo(next)
  }
  catch (e) {
    error.value = e instanceof ApiError ? e.message : '登录失败，请稍后再试'
  }
  finally {
    pending.value = false
  }
}
</script>

<template>
  <div class="relative flex min-h-dvh items-center justify-center overflow-hidden bg-background p-4">
    <div class="absolute top-1/2 left-1/2 size-[1600px] -translate-x-1/2 -translate-y-1/2 [mask-image:radial-gradient(560px_circle_at_center,white,transparent)]">
      <InspiraInteractiveGridPattern :squares="[40, 40]" />
    </div>
    <Card class="relative w-full max-w-sm shadow-lg">
      <CardHeader class="text-center">
        <span class="mx-auto mb-2 grid size-11 place-items-center rounded-lg bg-brand text-brand-foreground">
          <ShieldCheck class="size-6" />
        </span>
        <CardTitle>绳网管理后台</CardTitle>
        <CardDescription>用版主或管理员的论坛账号登录</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="space-y-4" @submit.prevent="submit">
          <div class="space-y-2">
            <Label for="identifier">用户名或邮箱</Label>
            <Input id="identifier" v-model="identifier" autocomplete="username" required />
          </div>
          <div class="space-y-2">
            <Label for="password">密码</Label>
            <Input id="password" v-model="password" type="password" autocomplete="current-password" required />
          </div>
          <p v-if="error" class="text-sm text-destructive" role="alert">
            {{ error }}
          </p>
          <Button type="submit" class="w-full" :disabled="pending">
            {{ pending ? '登录中…' : '登录' }}
          </Button>
        </form>
      </CardContent>
    </Card>
  </div>
</template>
