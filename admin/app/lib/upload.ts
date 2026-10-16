import { api, unwrap } from '~/lib/api'
import type { Schemas } from '~/lib/api'

export const IMAGE_ACCEPT = 'image/jpeg,image/png,image/gif,image/webp,image/avif'
const MAX_BYTES = 30 * 1024 * 1024

async function sha256Hex(file: File) {
  const digest = await crypto.subtle.digest('SHA-256', await file.arrayBuffer())
  return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('')
}

async function dimensions(file: File) {
  try {
    const bitmap = await createImageBitmap(file)
    const size = { width: bitmap.width, height: bitmap.height }
    bitmap.close()
    return size
  }
  catch {
    return { width: null, height: null }
  }
}

/**
 * 直传一张图片：签名 → 按返回的方法与请求头 PUT 到对象存储 → 完成校验。
 * 自己上传过相同内容时直接复用已有文件。
 */
export async function uploadImage(file: File): Promise<Schemas['MediaView']> {
  if (file.size === 0 || file.size > MAX_BYTES) throw new Error('图片大小需在 30MB 以内')
  const signed = await unwrap(api.POST('/v1/uploads/sign', {
    body: { filename: file.name, mimeType: file.type, size: file.size, contentHash: await sha256Hex(file) },
  }))
  if (signed.existing && signed.media) return signed.media
  if (!signed.uploadId || !signed.uploadUrl) throw new Error('上传签名缺少地址')
  const put = await fetch(signed.uploadUrl, {
    method: signed.method ?? 'PUT',
    headers: signed.headers ?? undefined,
    body: file,
  })
  if (!put.ok) throw new Error(`上传到存储失败（${put.status}）`)
  return unwrap(api.POST('/v1/uploads/{uploadId}/complete', {
    params: { path: { uploadId: signed.uploadId } },
    body: await dimensions(file),
  }))
}
