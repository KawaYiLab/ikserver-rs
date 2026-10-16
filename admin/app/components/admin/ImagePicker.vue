<script setup lang="ts">
import { ImageUp, LoaderCircle } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import type { Schemas } from '~/lib/api'
import { IMAGE_ACCEPT, uploadImage } from '~/lib/upload'

/** 选择并直传一张图片；`v-model` 是媒体文件。 */
const model = defineModel<Schemas['MediaView'] | null>()
defineProps<{ disabled?: boolean }>()

const { failure } = useNotify()
const input = ref<HTMLInputElement | null>(null)
const uploading = ref(false)

async function picked(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0]
  if (!file) return
  uploading.value = true
  try {
    model.value = await uploadImage(file)
  }
  catch (error) {
    failure(error, '上传失败')
  }
  finally {
    uploading.value = false
    if (input.value) input.value.value = ''
  }
}
</script>

<template>
  <div class="flex items-center gap-4">
    <div class="flex size-24 shrink-0 items-center justify-center overflow-hidden rounded-md border bg-muted">
      <img v-if="model" :src="model.thumbUrl" :alt="model.name" class="size-full object-cover">
      <span v-else class="text-xs text-muted-foreground">未选择</span>
    </div>
    <div class="space-y-1">
      <input ref="input" type="file" :accept="IMAGE_ACCEPT" class="hidden" @change="picked">
      <Button type="button" variant="outline" size="sm" :disabled="disabled || uploading" @click="input?.click()">
        <LoaderCircle v-if="uploading" class="size-4 animate-spin" />
        <ImageUp v-else class="size-4" />
        {{ model ? '更换图片' : '上传图片' }}
      </Button>
      <p class="text-xs text-muted-foreground">JPEG / PNG / GIF / WebP / AVIF，30MB 以内。</p>
    </div>
  </div>
</template>
