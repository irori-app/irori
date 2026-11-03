/**
 * Every page of the site in one list, for the Open Graph images and the section
 * indexes. A page's key is its path without slashes (`features/library`); the
 * home page's is `home`.
 */
import { getCollection, type CollectionEntry } from 'astro:content'
import { dictionary, type Locale } from '../i18n/config'

export type Section = 'features' | 'guides' | 'compare' | 'docs'

export interface PageInfo {
  key: string
  path: string
  title: string
  description: string
  /** The small label above the title on the Open Graph image. */
  tag: string
}

export function keyOf(path: string): string {
  return path.replace(/^\/|\/$/g, '') || 'home'
}

/** `en/library` → `library`. */
export function slugOf(id: string): string {
  return id.split('/').slice(1).join('/')
}

export function localeOf(id: string): string {
  return id.split('/')[0] ?? ''
}

/** A section's entries in one language, in their `order`. */
export async function entries<S extends Section>(section: S, locale: Locale): Promise<CollectionEntry<S>[]> {
  const all = (await getCollection(section)) as CollectionEntry<S>[]
  return all.filter((entry) => localeOf(entry.id) === locale).sort((a, b) => a.data.order - b.data.order)
}

const SECTION_TAGS: Record<Section, string> = { features: 'Features', guides: 'Guide', compare: 'Compare', docs: 'Docs' }

export async function allPages(locale: Locale): Promise<PageInfo[]> {
  const t = dictionary(locale)
  const pages: PageInfo[] = [
    { key: 'home', path: '/', title: t.site.tagline, description: t.home.description, tag: 'Game launcher' },
    { key: 'download', path: '/download/', title: t.download.h1, description: t.download.description, tag: 'Download' },
    { key: 'features', path: '/features/', title: t.features.h1, description: t.features.description, tag: 'Features' },
    { key: 'guides', path: '/guides/', title: t.guides.h1, description: t.guides.description, tag: 'Guides' },
    { key: 'compare', path: '/compare/', title: t.compare.h1, description: t.compare.description, tag: 'Compare' },
    { key: 'docs', path: '/docs/', title: t.docs.h1, description: t.docs.description, tag: 'Docs' },
    { key: 'changelog', path: '/changelog/', title: t.changelog.h1, description: t.changelog.description, tag: 'Changelog' },
    { key: 'code-signing', path: '/code-signing/', title: 'How Irori’s releases are signed', description: t.signing.description, tag: 'Code signing' },
  ]
  for (const section of ['features', 'guides', 'compare', 'docs'] as const) {
    for (const entry of await entries(section, locale)) {
      const path = `/${section}/${slugOf(entry.id)}/`
      pages.push({ key: keyOf(path), path, title: entry.data.title, description: entry.data.description, tag: SECTION_TAGS[section] })
    }
  }
  return pages
}

/** The cards at the end of an article: its `related` slugs, from the same section. */
export async function relatedOf(section: Section, slugs: string[], locale: Locale) {
  const list = await entries(section, locale)
  return slugs
    .map((slug) => list.find((entry) => slugOf(entry.id) === slug))
    .filter((entry): entry is NonNullable<typeof entry> => Boolean(entry))
    .map((entry) => ({
      title: entry.data.title,
      summary: (entry.data as { summary?: string }).summary ?? entry.data.description,
      path: `/${section}/${slugOf(entry.id)}/`,
      tag: (entry.data as { tag?: string }).tag ?? SECTION_TAGS[section],
    }))
}
