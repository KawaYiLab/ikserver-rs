import tailwindcss from '@tailwindcss/vite'

// 管理后台是纯 SPA：`nuxt generate` 的产物编进 ik-server，在 API 域名（ik.tiwat.cn）的 /admin 下提供，
// 接口走同源的 /v1（会话 Cookie）。
export default defineNuxtConfig({
  compatibilityDate: '2026-08-01',
  ssr: false,
  devtools: { enabled: false },
  modules: ['shadcn-nuxt', 'motion-v/nuxt'],
  css: ['@fontsource-variable/inter', '~/assets/css/main.css'],
  app: {
    baseURL: '/admin/',
    pageTransition: { name: 'page', mode: 'out-in' },
    head: {
      title: '绳网管理后台',
      htmlAttrs: { lang: 'zh-CN' },
      meta: [{ name: 'robots', content: 'noindex, nofollow' }],
    },
  },
  runtimeConfig: {
    public: {
      // 主站地址，用于「查看帖子 / 主页」等链接（RPTADM-015）。构建时用 NUXT_PUBLIC_SITE_URL 覆盖。
      siteUrl: 'https://interk.net',
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
