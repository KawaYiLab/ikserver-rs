<script setup lang="ts">
import { Ban, Plus, ShieldBan, Trash2, UserCheck, UserPlus } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import type { Schemas } from '~/lib/api'
import { getPath, newUserFields } from '~/lib/settings'

definePageMeta({ admin: true })

type Word = Schemas['WatchedWord']
type IpBan = Schemas['IpBan']

const listPaths = ['filter.words', 'registration.emailDomainBlocklist', 'registration.reservedUsernames', 'ipBans']
const { settings, draft, errors, loading, saving, dirtyPaths, load, value, update, discard, save }
  = useSiteSettings([...listPaths, ...newUserFields.map(field => field.path)])

const words = computed(() => (value('filter.words') ?? []) as Word[])
function wordsOf(action: Word['action']) {
  return words.value.filter(w => w.action === action).map(w => w.word)
}
function setWords(action: Word['action'], list: string[]) {
  const other = words.value.filter(w => w.action !== action)
  const next = list.map(word => ({ word, action }))
  update('filter.words', action === 'block' ? [...next, ...other] : [...other, ...next])
}

const bans = computed(() => (value('ipBans') ?? []) as IpBan[])
function setBan(index: number, patch: Partial<IpBan>) {
  update('ipBans', bans.value.map((ban, i) => (i === index ? { ...ban, ...patch } : ban)))
}
function addBan() {
  update('ipBans', [...bans.value, { cidr: '', note: '', expiresAt: null }])
}
function removeBan(index: number) {
  update('ipBans', bans.value.filter((_, i) => i !== index))
}
function expired(ban: IpBan) {
  return !!ban.expiresAt && new Date(ban.expiresAt).getTime() <= Date.now()
}
// datetime-local 用本地时间；接口用 ISO 时间，空为永久。
function toLocalInput(value?: string | null) {
  if (!value) return ''
  const date = new Date(value)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}
</script>

<template>
  <div class="pb-24">
    <AdminPageHeader title="内容与安全" description="屏蔽词、注册限制、新账号限制与 IP 封禁。版主、管理员与 AI 账号不受这些限制。">
      <AdminRefreshButton :loading="loading" @click="load" />
    </AdminPageHeader>

    <div v-if="!draft || !settings" class="space-y-6">
      <Skeleton v-for="i in 4" :key="i" class="h-48 w-full rounded-xl" />
    </div>

    <div v-else class="grid grid-cols-1 gap-6 xl:grid-cols-2">
      <Card class="min-w-0 gap-0 py-0 shadow-xs xl:col-span-2">
        <CardHeader class="border-b py-5">
          <CardTitle class="flex items-center gap-2 text-base">
            <Ban class="size-4 text-muted-foreground" />屏蔽词
          </CardTitle>
          <CardDescription>作用于帖子标题与正文、评论和改名。按字面匹配，英文不区分大小写，每行一个。</CardDescription>
        </CardHeader>
        <CardContent class="grid gap-5 py-5 md:grid-cols-2">
          <div class="space-y-2">
            <p class="text-sm font-medium">
              拦截
            </p>
            <p class="text-xs text-muted-foreground">
              命中时拒绝提交，并提示命中的词。
            </p>
            <AdminListTextarea
              :model-value="wordsOf('block')" :disabled="saving" :invalid="!!errors['filter.words']" :rows="6"
              @update:model-value="setWords('block', $event)"
            />
          </div>
          <div class="space-y-2">
            <p class="text-sm font-medium">
              替换为 *
            </p>
            <p class="text-xs text-muted-foreground">
              命中时替换成等长的 * 后照常保存。
            </p>
            <AdminListTextarea
              :model-value="wordsOf('censor')" :disabled="saving" :invalid="!!errors['filter.words']" :rows="6"
              @update:model-value="setWords('censor', $event)"
            />
          </div>
          <p v-if="errors['filter.words']" class="text-xs text-destructive md:col-span-2" role="alert">
            {{ errors['filter.words'] }}
          </p>
        </CardContent>
      </Card>

      <Card class="min-w-0 gap-0 py-0 shadow-xs">
        <CardHeader class="border-b py-5">
          <CardTitle class="flex items-center gap-2 text-base">
            <UserPlus class="size-4 text-muted-foreground" />注册限制
          </CardTitle>
          <CardDescription>已有账号不受影响。</CardDescription>
        </CardHeader>
        <CardContent class="space-y-5 py-5">
          <div class="space-y-2">
            <p class="text-sm font-medium">
              禁止注册的邮箱域名
            </p>
            <p class="text-xs text-muted-foreground">
              也会拦它的子域名，每行一个。
            </p>
            <AdminListTextarea
              :model-value="value('registration.emailDomainBlocklist') as string[]" lowercase placeholder="mailinator.com"
              :disabled="saving" :invalid="!!errors['registration.emailDomainBlocklist']"
              @update:model-value="update('registration.emailDomainBlocklist', $event)"
            />
            <p v-if="errors['registration.emailDomainBlocklist']" class="text-xs text-destructive" role="alert">
              {{ errors['registration.emailDomainBlocklist'] }}
            </p>
          </div>
          <div class="space-y-2">
            <p class="text-sm font-medium">
              保留用户名
            </p>
            <p class="text-xs text-muted-foreground">
              用户不能改成这些名字，整名匹配、不区分大小写；后台改名不受限制。
            </p>
            <AdminListTextarea
              :model-value="value('registration.reservedUsernames') as string[]" placeholder="admin"
              :disabled="saving" :invalid="!!errors['registration.reservedUsernames']"
              @update:model-value="update('registration.reservedUsernames', $event)"
            />
            <p v-if="errors['registration.reservedUsernames']" class="text-xs text-destructive" role="alert">
              {{ errors['registration.reservedUsernames'] }}
            </p>
          </div>
        </CardContent>
      </Card>

      <Card class="min-w-0 gap-0 py-0 shadow-xs">
        <CardHeader class="border-b py-5">
          <CardTitle class="flex items-center gap-2 text-base">
            <UserCheck class="size-4 text-muted-foreground" />新账号限制
          </CardTitle>
          <CardDescription>刚注册的账号在一段时间内发得少一点，挡住大部分广告号。</CardDescription>
        </CardHeader>
        <CardContent class="divide-y py-0">
          <AdminSettingField
            v-for="field in newUserFields" :key="field.path"
            :field="field"
            :model-value="value(field.path)"
            :default-value="getPath(settings.defaults, field.path)"
            :dirty="dirtyPaths.includes(field.path)"
            :error="errors[field.path]"
            :disabled="saving"
            @update:model-value="update(field.path, $event)"
          />
        </CardContent>
      </Card>

      <Card class="min-w-0 gap-0 py-0 shadow-xs xl:col-span-2">
        <CardHeader class="border-b py-5">
          <div class="flex items-start justify-between gap-4">
            <div class="space-y-1.5">
              <CardTitle class="flex items-center gap-2 text-base">
                <ShieldBan class="size-4 text-muted-foreground" />IP 封禁
              </CardTitle>
              <CardDescription>命中的地址不能注册、登录和执行写操作，浏览不受影响。支持单个地址或网段。</CardDescription>
            </div>
            <Button variant="outline" size="sm" :disabled="saving" @click="addBan">
              <Plus />添加
            </Button>
          </div>
        </CardHeader>
        <CardContent class="p-0">
          <Table v-if="bans.length">
            <TableHeader>
              <TableRow>
                <TableHead class="w-56 pl-6">
                  地址或网段
                </TableHead>
                <TableHead>备注</TableHead>
                <TableHead class="w-60">
                  到期时间
                </TableHead>
                <TableHead class="w-14 pr-6" />
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="(ban, index) in bans" :key="index" :class="expired(ban) && 'text-muted-foreground'">
                <TableCell class="pl-6">
                  <Input
                    :model-value="ban.cidr" placeholder="203.0.113.0/24" class="font-mono" :disabled="saving"
                    :aria-label="`第 ${index + 1} 条的地址`"
                    @update:model-value="setBan(index, { cidr: String($event).trim() })"
                  />
                </TableCell>
                <TableCell>
                  <Input
                    :model-value="ban.note" placeholder="原因" maxlength="100" :disabled="saving"
                    :aria-label="`第 ${index + 1} 条的备注`"
                    @update:model-value="setBan(index, { note: String($event) })"
                  />
                </TableCell>
                <TableCell>
                  <div class="flex items-center gap-2">
                    <Input
                      type="datetime-local" :model-value="toLocalInput(ban.expiresAt)" :disabled="saving"
                      :aria-label="`第 ${index + 1} 条的到期时间`"
                      @update:model-value="setBan(index, { expiresAt: $event ? new Date(String($event)).toISOString() : null })"
                    />
                    <span v-if="expired(ban)" class="shrink-0 text-xs">已过期</span>
                  </div>
                </TableCell>
                <TableCell class="pr-6">
                  <Button
                    variant="ghost" size="icon-sm" class="text-muted-foreground" :disabled="saving"
                    :aria-label="`删除第 ${index + 1} 条`" @click="removeBan(index)"
                  >
                    <Trash2 />
                  </Button>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <p v-else class="px-6 py-10 text-center text-sm text-muted-foreground">
            还没有封禁的地址
          </p>
          <p v-if="errors.ipBans" class="border-t px-6 py-3 text-xs text-destructive" role="alert">
            {{ errors.ipBans }}
          </p>
        </CardContent>
      </Card>
    </div>

    <AdminSaveBar :count="dirtyPaths.length" :saving="saving" @save="save" @discard="discard" />
  </div>
</template>
