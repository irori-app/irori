// Irori's website: static pages, prerendered by Astro, with Vue for the few interactive parts.
import sitemap from '@astrojs/sitemap'
import vue from '@astrojs/vue'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'astro/config'

export default defineConfig({
  site: 'https://irori.app',
  // Pages are folders (`/features/library/`), which Caddy serves without rewrites.
  trailingSlash: 'always',
  // One request less before the first paint; the CSS is small once compressed.
  build: { format: 'directory', inlineStylesheets: 'always' },
  i18n: {
    // Add a locale here, a dictionary in `src/i18n/` and content in `src/content/*/<locale>/`.
    locales: ['en'],
    defaultLocale: 'en',
    routing: { prefixDefaultLocale: false },
  },
  integrations: [
    vue(),
    sitemap({
      filter: (page) => !page.includes('/og/') && !page.includes('/covers/'),
    }),
  ],
  vite: { plugins: [tailwindcss()] },
  devToolbar: { enabled: false },
  prefetch: { prefetchAll: false, defaultStrategy: 'hover' },
})
