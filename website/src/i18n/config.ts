/**
 * The site's languages. English lives at the root (`/features/`); every other
 * language gets a prefix (`/de/features/`). To add one, list it here and in
 * `astro.config.mjs`, add its dictionary to `DICTIONARIES`, put its pages under
 * `src/pages/[lang]/` (each renders the same view as the English page) and its
 * articles under `src/content/<collection>/<locale>/`.
 */
import { en, type Dictionary } from './en'

export const LOCALES = ['en'] as const
export type Locale = (typeof LOCALES)[number]
export const DEFAULT_LOCALE: Locale = 'en'

const DICTIONARIES: Record<Locale, Dictionary> = { en }

/** BCP 47 tags for `<html lang>`, `og:locale` and `hreflang`. */
export const LANG_TAGS: Record<Locale, { html: string; og: string }> = {
  en: { html: 'en', og: 'en_US' },
}

export function dictionary(locale: Locale): Dictionary {
  return DICTIONARIES[locale]
}

/** The path of a page in a language: `localePath('de', '/docs/')` is `/de/docs/`. */
export function localePath(locale: Locale, path: string): string {
  const clean = path.startsWith('/') ? path : `/${path}`
  return locale === DEFAULT_LOCALE ? clean : `/${locale}${clean}`
}

/** Fills `{name}` placeholders. */
export function format(text: string, values: Record<string, string | number>): string {
  return text.replace(/\{(\w+)\}/g, (match, key: string) => (key in values ? String(values[key]) : match))
}
