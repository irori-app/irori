// Downloads the portrait covers listed in src/data/game-covers.json from Steam's public
// library art, once, and stores them in public/covers/games/ as AVIF and WebP at 300 and
// 600 pixels wide. It also fills in each cover's `accent` (its average colour), which the
// cover wall uses for the hover glow. The site serves these files itself, so visitors
// never load anything from Steam.
//
//   node scripts/fetch-covers.mjs            fetch covers that are missing
//   node scripts/fetch-covers.mjs --force    fetch them all again
//
// It exits with an error naming every app id that returned no image.
import { existsSync } from 'node:fs'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import sharp from 'sharp'

const root = join(import.meta.dirname, '..')
const listPath = join(root, 'src/data/game-covers.json')
const outDir = join(root, 'public/covers/games')
const force = process.argv.includes('--force')

const sources = (appid) => [
  `https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/${appid}/library_600x900_2x.jpg`,
  `https://cdn.cloudflare.steamstatic.com/steam/apps/${appid}/library_600x900_2x.jpg`,
  `https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/${appid}/library_600x900.jpg`,
  `https://cdn.cloudflare.steamstatic.com/steam/apps/${appid}/library_600x900.jpg`,
]

async function download(appid) {
  for (const url of sources(appid)) {
    const response = await fetch(url, { signal: AbortSignal.timeout(20_000) }).catch(() => null)
    if (!response?.ok || !response.headers.get('content-type')?.startsWith('image/')) continue
    return Buffer.from(await response.arrayBuffer())
  }
  return null
}

const hex = (n) => Math.round(n).toString(16).padStart(2, '0')

const games = JSON.parse(await readFile(listPath, 'utf8'))
await mkdir(outDir, { recursive: true })
const missing = []

for (const game of games) {
  if (!/^[a-z0-9-]+$/.test(game.file)) throw new Error(`${game.title}: "file" may only hold a-z, 0-9 and -`)
  const done = [300, 600].every((w) => existsSync(join(outDir, `${game.file}-${w}.webp`)) && existsSync(join(outDir, `${game.file}-${w}.avif`)))
  if (done && game.accent && !force) continue

  const image = await download(game.appid)
  if (!image) {
    missing.push(`${game.title} (${game.appid})`)
    continue
  }
  // Every cover is cut to 2:3, whatever size Steam sent.
  const base = sharp(image).resize(600, 900, { fit: 'cover' })
  for (const width of [300, 600]) {
    const sized = base.clone().resize(width, width * 1.5)
    await sized.clone().webp({ quality: 78 }).toFile(join(outDir, `${game.file}-${width}.webp`))
    await sized.clone().avif({ quality: 55 }).toFile(join(outDir, `${game.file}-${width}.avif`))
  }
  const { channels } = await sharp(image).stats()
  game.accent = `#${channels.slice(0, 3).map((c) => hex(c.mean)).join('')}`
  console.log(`✓ ${game.title}`)
}

await writeFile(listPath, `${JSON.stringify(games, null, 2)}\n`)
if (missing.length) {
  console.error(`No image for: ${missing.join(', ')}. Fix their app ids in src/data/game-covers.json.`)
  process.exit(1)
}
