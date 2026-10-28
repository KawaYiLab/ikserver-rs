<script setup lang="ts">
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ApiError, api, unwrap } from '~/lib/api'

definePageMeta({ layout: false })

const { check, markDone } = useSetup()
const { login } = useSession()
const token = ref('')
const email = ref('')
const username = ref('')
const password = ref('')
const confirm = ref('')
const pending = ref(false)
const error = ref('')

onMounted(async () => {
  if (!(await check())) await navigateTo('/login')
})

async function submit() {
  error.value = ''
  if (password.value !== confirm.value) {
    error.value = '两次输入的密码不一样'
    return
  }
  pending.value = true
  try {
    await unwrap(api.POST('/v1/setup', {
      body: { token: token.value.trim(), email: email.value.trim(), username: username.value.trim(), password: password.value },
    }))
    markDone()
    await login(email.value.trim(), password.value)
    await navigateTo('/settings')
  }
  catch (e) {
    if (e instanceof ApiError && e.code === 'ALREADY_SET_UP') {
      markDone()
      await navigateTo('/login')
      return
    }
    error.value = e instanceof ApiError ? e.message : '安装失败，请稍后再试'
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
        <AdminBrandMark class="mx-auto mb-2 size-11 rounded-xl" />
        <CardTitle>创建管理员</CardTitle>
        <CardDescription>第一次使用，先建一个管理员账号</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="space-y-4" @submit.prevent="submit">
          <div class="space-y-2">
            <Label for="token">安装码</Label>
            <Input id="token" v-model="token" autocomplete="off" spellcheck="false" class="font-mono" required />
            <p class="text-xs text-muted-foreground">
              在服务的启动日志里
            </p>
          </div>
          <div class="space-y-2">
            <Label for="email">邮箱</Label>
            <Input id="email" v-model="email" type="email" autocomplete="email" required />
          </div>
          <div class="space-y-2">
            <Label for="username">用户名</Label>
            <Input id="username" v-model="username" autocomplete="username" minlength="3" maxlength="20" required />
          </div>
          <div class="space-y-2">
            <Label for="password">密码</Label>
            <Input id="password" v-model="password" type="password" autocomplete="new-password" required />
          </div>
          <div class="space-y-2">
            <Label for="confirm">再输一次密码</Label>
            <Input id="confirm" v-model="confirm" type="password" autocomplete="new-password" required />
          </div>
          <p v-if="error" class="text-sm text-destructive" role="alert">
            {{ error }}
          </p>
          <Button type="submit" class="w-full" :disabled="pending">
            {{ pending ? '创建中…' : '创建并登录' }}
          </Button>
        </form>
      </CardContent>
    </Card>
  </div>
</template>
