/**
 * What the cover walls show. Real game covers (`src/data/game-covers.json`, fetched with
 * `scripts/fetch-covers.mjs`) are used once every listed cover has its files; until then
 * the walls show the demo library's drawn covers. Store tiles are mixed in either way.
 */
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { COVERS } from '../data/covers'
import games from '../data/game-covers.json'

export type WallItem =
  | {
      kind: 'cover'
      slug: string
      title: string
      src: string
      srcset?: string
      /** AVIF twin of `srcset`, offered first where the browser takes it. */
      avifset?: string
      accent: string
      face?: 'display' | 'serif' | 'plain'
      titleInArt?: boolean
    }
  | { kind: 'store'; name: string }

/** Stores shown as tiles among the covers, one every few covers. */
const STORES = ['Epic Games', 'GOG', 'Steam', 'itch.io', 'Amazon Games', 'ZOOM Platform']
const STORE_EVERY = 6

const gamesDir = join(process.cwd(), 'public/covers/games')

/** Whether the real covers are on disk, so the site uses them and credits their owners. */
export function usesGameCovers(): boolean {
  return games.length > 0 && games.every((g) => existsSync(join(gamesDir, `${g.file}-600.webp`)) && 'accent' in g)
}

export function wallItems(): WallItem[] {
  const covers: WallItem[] = usesGameCovers()
    ? games.map((g) => ({
        kind: 'cover',
        slug: g.file,
        title: g.title,
        src: `/covers/games/${g.file}-600.webp`,
        srcset: `/covers/games/${g.file}-300.webp 300w, /covers/games/${g.file}-600.webp 600w`,
        avifset: `/covers/games/${g.file}-300.avif 300w, /covers/games/${g.file}-600.avif 600w`,
        accent: (g as { accent?: string }).accent ?? '#ff6b2c',
        titleInArt: true,
      }))
    : COVERS.map((c) => ({
        kind: 'cover',
        slug: c.slug,
        title: c.title,
        src: c.src ?? `/covers/${c.slug}.svg`,
        accent: c.accent,
        face: c.face,
        titleInArt: c.titleInArt,
      }))
  const items: WallItem[] = []
  covers.forEach((cover, i) => {
    items.push(cover)
    if (i % STORE_EVERY === STORE_EVERY - 1) items.push({ kind: 'store', name: STORES[Math.floor(i / STORE_EVERY) % STORES.length]! })
  })
  return items
}
