import tailwindcss from '@tailwindcss/vite'

// 管理后台是纯 SPA：nginx 在 /admin 下托管 `nuxt generate` 的产物，接口走同源的 /v1（会话 Cookie）。
export default defineNuxtConfig({
  compatibilityDate: '2026-08-01',
  ssr: false,
  devtools: { enabled: false },
  modules: ['shadcn-nuxt'],
  css: ['~/assets/css/main.css'],
  app: {
    baseURL: '/admin/',
    head: {
      title: '绳网管理后台',
      htmlAttrs: { lang: 'zh-CN' },
      meta: [{ name: 'robots', content: 'noindex, nofollow' }],
    },
  },
  shadcn: {
    prefix: '',
    componentDir: './app/components/ui',
  },
  vite: {
    plugins: [tailwindcss()],
  },
  // 本地开发：把 /v1 转给本机的 ik-server（默认 :8080），保持同源，Cookie 与 CSRF 校验照常生效。
  nitro: {
    devProxy: {
      '/v1': { target: 'http://127.0.0.1:8080/v1', changeOrigin: false },
    },
  },
  typescript: {
    strict: true,
  },
})
