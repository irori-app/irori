// Render the home page's handheld (Big Picture on a drawn device) to
// public/screenshots/bigpicture-handheld.webp, the picture the README shows.
//
// Usage, from website/ after `bun run build` and with the site served on a
// local port:  bun scripts/screenshots/handheld.mjs http://localhost:4325/ out.png
// then: cwebp -q 86 out.png -o public/screenshots/bigpicture-handheld.webp
//
// It drives Google Chrome headless over the DevTools protocol with Bun's own
// WebSocket, so it needs no npm package.
import { spawn } from 'node:child_process'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const [url, out] = process.argv.slice(2)
if (!url || !out) {
  console.error('usage: handheld.mjs <site url> <out.png>')
  process.exit(1)
}
const chrome = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const port = 9333
const profile = mkdtempSync(join(tmpdir(), 'irori-handheld-'))
const proc = spawn(chrome, [
  '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  '--hide-scrollbars', '--force-prefers-reduced-motion', 'about:blank',
], { stdio: 'ignore' })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
let target
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200)
  try {
    const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json()
    target = list.find((t) => t.type === 'page')
  } catch {}
}
if (!target) throw new Error('Chrome did not start')

const ws = new WebSocket(target.webSocketDebuggerUrl)
await new Promise((r) => ws.addEventListener('open', r, { once: true }))
let id = 0
const pending = new Map()
ws.addEventListener('message', (e) => {
  const msg = JSON.parse(e.data)
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
  }
})
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const n = ++id
    pending.set(n, (m) => (m.error ? reject(new Error(m.error.message)) : resolve(m.result)))
    ws.send(JSON.stringify({ id: n, method, params }))
  })

await send('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1000, deviceScaleFactor: 2, mobile: false })
await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] })
await send('Page.enable')
await send('Page.navigate', { url })
await sleep(4000)
// Show the Irori carousel, keep only the device, and let its images load.
const { result } = await send('Runtime.evaluate', {
  awaitPromise: true,
  returnByValue: true,
  expression: `(async () => {
    const stage = document.querySelector('.stage')
    stage.scrollIntoView({ block: 'center' })
    const chip = [...document.querySelectorAll('button')].find((b) => /carousel/i.test(b.textContent) && /irori/i.test(b.textContent))
    chip?.click()
    await new Promise((r) => setTimeout(r, 2500))
    document.documentElement.style.background = 'transparent'
    document.body.style.background = 'transparent'
    for (const el of document.querySelectorAll('.aura')) el.style.display = 'none'
    for (let el = stage; el; el = el.parentElement) {
      el.style.background = 'transparent'
      el.style.backgroundImage = 'none'
    }
    for (const el of stage.parentElement.querySelectorAll(':scope > *:not(.stage)')) el.style.visibility = 'hidden'
    const device = stage.querySelector('.device')
    device.style.transform = 'none'
    await Promise.all([...device.querySelectorAll('img')].map((i) => i.decode().catch(() => {})))
    const r = device.getBoundingClientRect()
    return { x: r.x + scrollX, y: r.y + scrollY, width: r.width, height: r.height }
  })()`,
})
const box = result.value
const pad = 24
await send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 0, g: 0, b: 0, a: 0 } })
// Hide the rest of the page so only the device and its shadow are drawn.
await send('Runtime.evaluate', {
  expression: `for (const el of document.body.querySelectorAll('body *')) {
    if (!el.closest('.stage') && !el.querySelector('.stage')) el.style.visibility = 'hidden'
  }`,
})
const shot = await send('Page.captureScreenshot', {
  format: 'png',
  captureBeyondViewport: true,
  clip: { x: box.x - pad, y: box.y - pad, width: box.width + pad * 2, height: box.height + pad * 3, scale: 1 },
})
writeFileSync(out, Buffer.from(shot.data, 'base64'))
ws.close()
proc.kill()
console.log(`wrote ${out}`)
