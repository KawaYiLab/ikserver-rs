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
      error.value = '这个账号没有后台权限（需要版主或管理员）'
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
  <div class="flex min-h-dvh items-center justify-center bg-muted/40 p-4">
    <Card class="w-full max-w-sm">
      <CardHeader class="text-center">
        <ShieldCheck class="mx-auto mb-2 size-8" />
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
