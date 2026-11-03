/**
 * Draws a page's Open Graph image, 1200 × 630: the page's title in Irori's type
 * on the night hearth, with three demo covers fanned out on the right.
 */
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { join } from 'node:path'
import { Resvg } from '@resvg/resvg-js'
import satori from 'satori'
import { COVERS } from '../data/covers'
import { coverSvg } from './cover-svg'
import type { PageInfo } from './pages'

// Resolved from the site's folder, as the build moves this module elsewhere.
const require = createRequire(join(process.cwd(), 'package.json'))
const font = (pkg: string, file: string) => readFile(require.resolve(`@fontsource/${pkg}/files/${file}`))

let fonts: Promise<{ name: string; data: Buffer; weight: 400 | 500 | 700; style: 'normal' }[]> | undefined
function loadFonts() {
  fonts ??= Promise.all([
    font('zen-maru-gothic', 'zen-maru-gothic-latin-700-normal.woff').then((data) => ({ name: 'Display', data, weight: 700 as const, style: 'normal' as const })),
    font('zen-kaku-gothic-new', 'zen-kaku-gothic-new-latin-500-normal.woff').then((data) => ({ name: 'Body', data, weight: 500 as const, style: 'normal' as const })),
    font('m-plus-1-code', 'm-plus-1-code-latin-500-normal.woff').then((data) => ({ name: 'Code', data, weight: 500 as const, style: 'normal' as const })),
  ])
  return fonts
}

let mark: Promise<string> | undefined
function loadMark() {
  mark ??= readFile(join(process.cwd(), 'public/brand/irori-mark.svg'), 'utf8').then(dataUri)
  return mark
}

const dataUri = (svg: string) => `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`

/** A small element builder, so the layout reads as markup without JSX. */
type Node = { type: string; props: Record<string, unknown> }
const el = (type: string, style: Record<string, unknown>, children?: unknown, extra: Record<string, unknown> = {}): Node => ({
  type,
  props: { style, children, ...extra },
})

/** Picks three covers from the page's key, so neighbouring pages look different. */
function coversFor(key: string) {
  let hash = 0
  for (const c of key) hash = (hash * 31 + c.charCodeAt(0)) >>> 0
  return [0, 1, 2].map((i) => COVERS[(hash + i * 7) % COVERS.length]!)
}

export async function ogImage(page: PageInfo): Promise<Buffer> {
  const [fontList, markUri] = await Promise.all([loadFonts(), loadMark()])
  const covers = coversFor(page.key)
  const titleSize = page.title.length > 48 ? 58 : page.title.length > 30 ? 68 : 80

  const tree = el(
    'div',
    {
      width: 1200,
      height: 630,
      display: 'flex',
      position: 'relative',
      background: '#15110e',
      backgroundImage: 'radial-gradient(circle at 18% 105%, rgba(255,107,44,0.38), rgba(255,107,44,0) 55%)',
      fontFamily: 'Body',
      color: '#f2e9dc',
      overflow: 'hidden',
    },
    [
      ...covers.map((cover, i) =>
        el('img', {
          position: 'absolute',
          width: 260,
          height: 390,
          left: 780 + i * 120,
          top: 70 + i * 40,
          borderRadius: 22,
          transform: `rotate(${-10 + i * 9}deg)`,
          boxShadow: '0 30px 60px rgba(0,0,0,0.55)',
          opacity: i === 1 ? 1 : 0.9,
        }, undefined, { src: dataUri(coverSvg(cover)), width: 260, height: 390 }),
      ),
      el('div', {
        position: 'absolute',
        inset: 0,
        display: 'flex',
        backgroundImage: 'linear-gradient(90deg, #15110e 0%, #15110e 52%, rgba(21,17,14,0.6) 70%, rgba(21,17,14,0) 100%)',
      }),
      el(
        'div',
        { position: 'absolute', left: 72, top: 64, bottom: 64, width: 720, display: 'flex', flexDirection: 'column' },
        [
          el('div', { display: 'flex', alignItems: 'center', gap: 18 }, [
            el('img', { width: 64, height: 64 }, undefined, { src: markUri, width: 64, height: 64 }),
            el('div', { fontFamily: 'Display', fontSize: 40, color: '#f2e9dc' }, 'Irori'),
            el('div', { fontFamily: 'Code', fontSize: 22, color: '#a9b47c', letterSpacing: 4, marginLeft: 12, textTransform: 'uppercase' }, page.tag),
          ]),
          el('div', { display: 'flex', flexDirection: 'column', marginTop: 'auto', gap: 22 }, [
            el('div', { fontFamily: 'Display', fontSize: titleSize, lineHeight: 1.04, letterSpacing: -1.5, color: '#f2e9dc' }, page.title),
            el('div', { fontSize: 27, lineHeight: 1.4, color: '#b2a493', display: 'flex' }, page.description.length > 140 ? `${page.description.slice(0, 137).trimEnd()}…` : page.description),
          ]),
          el('div', { display: 'flex', alignItems: 'center', gap: 12, marginTop: 34, fontFamily: 'Code', fontSize: 22, color: '#ff6b2c' }, [
            el('div', { width: 12, height: 12, borderRadius: 6, background: '#ff6b2c' }),
            'irori.app',
          ]),
        ],
      ),
    ],
  )

  const svg = await satori(tree as never, { width: 1200, height: 630, fonts: fontList })
  return new Resvg(svg, { fitTo: { mode: 'width', value: 1200 } }).render().asPng()
}
