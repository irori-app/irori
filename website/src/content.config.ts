/**
 * Long-form pages, one Markdown file per page and language:
 * `src/content/<collection>/<locale>/<slug>.md`.
 */
import { defineCollection } from 'astro:content'
import { glob } from 'astro/loaders'
import { z } from 'astro/zod'

const base = z.object({
  /** The page's heading. */
  title: z.string(),
  /** The `<title>`, keyword-rich; " | Irori" is added. */
  seoTitle: z.string(),
  description: z.string().max(165),
  order: z.number(),
  updated: z.coerce.date(),
  related: z.array(z.string()).default([]),
})

const features = defineCollection({
  loader: glob({ pattern: '**/*.md', base: './src/content/features' }),
  schema: base.extend({
    tag: z.string(),
    /** One line for the card on the features index and the home page. */
    summary: z.string(),
    icon: z.string(),
    screen: z.string().optional(),
    highlights: z.array(z.object({ title: z.string(), body: z.string() })).default([]),
  }),
})

const guides = defineCollection({
  loader: glob({ pattern: '**/*.md', base: './src/content/guides' }),
  schema: base.extend({
    summary: z.string(),
    platforms: z.array(z.enum(['linux', 'macos', 'windows', 'steamdeck'])),
  }),
})

const compare = defineCollection({
  loader: glob({ pattern: '**/*.md', base: './src/content/compare' }),
  schema: base.extend({
    rival: z.string(),
    summary: z.string(),
    rows: z.array(
      z.object({
        feature: z.string(),
        irori: z.string(),
        rival: z.string(),
      }),
    ),
  }),
})

const docs = defineCollection({
  loader: glob({ pattern: '**/*.md', base: './src/content/docs' }),
  schema: base.extend({ summary: z.string() }),
})

export const collections = { features, guides, compare, docs }
