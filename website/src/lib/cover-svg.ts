/** Draws a demo cover as SVG: a sky, one motif and a darkened foot for the title. The same slug always draws the same picture. */

import type { Cover } from '../data/covers'

const W = 600
const H = 900

/** A small seeded generator, so stars and tiles land in the same place on every build. */
function random(seed: string) {
  let h = 2166136261
  for (const c of seed) h = Math.imul(h ^ c.charCodeAt(0), 16777619)
  return () => {
    h = Math.imul(h ^ (h >>> 15), 2246822507)
    h = Math.imul(h ^ (h >>> 13), 3266489909)
    return ((h ^= h >>> 16) >>> 0) / 4294967296
  }
}

function motif(cover: Cover, rand: () => number): string {
  const { accent: a, ground: g } = cover
  switch (cover.motif) {
    case 'sun':
      return `<circle cx="300" cy="420" r="190" fill="${a}"/>
        ${[0, 1, 2, 3, 4].map((i) => `<rect x="0" y="${470 + i * 34}" width="${W}" height="${8 + i * 3}" fill="${g}"/>`).join('')}
        <rect x="0" y="640" width="${W}" height="260" fill="${g}"/>`
    case 'moon': {
      const ridge = (y: number, amp: number, fill: string, o: number) =>
        `<path d="M0 ${y} L90 ${y - amp} L170 ${y - amp / 3} L260 ${y - amp * 1.3} L360 ${y - amp / 2} L450 ${y - amp * 1.1} L600 ${y - amp / 4} V900 H0Z" fill="${fill}" opacity="${o}"/>`
      return `<circle cx="420" cy="250" r="54" fill="${a}"/>
        ${ridge(520, 120, g, 0.55)}${ridge(610, 150, g, 0.8)}${ridge(700, 110, g, 1)}`
    }
    case 'waves':
      return [0, 1, 2, 3, 4, 5]
        .map((i) => {
          const y = 330 + i * 70
          let d = `M0 ${y}`
          for (let x = 0; x <= W; x += 75) d += ` Q${x + 37.5} ${y - 22} ${x + 75} ${y}`
          return `<path d="${d} V900 H0Z" fill="${i === 0 ? a : g}" opacity="${0.35 + i * 0.12}"/>`
        })
        .join('')
    case 'orbits':
      return `${[150, 210, 270, 330].map((r) => `<ellipse cx="300" cy="390" rx="${r * 1.25}" ry="${r * 0.62}" fill="none" stroke="${a}" stroke-opacity="0.35" stroke-width="3" transform="rotate(-14 300 390)"/>`).join('')}
        <circle cx="300" cy="390" r="62" fill="${a}"/>
        ${Array.from({ length: 6 }, () => `<circle cx="${40 + rand() * 520}" cy="${120 + rand() * 520}" r="${4 + rand() * 6}" fill="${a}"/>`).join('')}`
    case 'grid': {
      const lines = Array.from({ length: 9 }, (_, i) => {
        const y = 520 + (i * i * 380) / 64
        return `<line x1="0" y1="${y}" x2="${W}" y2="${y}" stroke="${a}" stroke-opacity="0.45" stroke-width="2"/>`
      }).join('')
      const rays = Array.from({ length: 13 }, (_, i) => `<line x1="300" y1="520" x2="${-600 + i * 150}" y2="900" stroke="${a}" stroke-opacity="0.4" stroke-width="2"/>`).join('')
      return `<circle cx="300" cy="430" r="150" fill="${a}" opacity="0.92"/>
        <rect x="0" y="520" width="${W}" height="380" fill="${g}"/>${lines}${rays}`
    }
    case 'mosaic': {
      const tiles: string[] = []
      for (let y = 60; y < 720; y += 66)
        for (let x = 30; x < 570; x += 66)
          if (rand() < 0.42) tiles.push(`<rect x="${x}" y="${y}" width="56" height="56" rx="6" fill="${rand() < 0.3 ? a : g}" opacity="${0.55 + rand() * 0.45}"/>`)
      return tiles.join('')
    }
    case 'beam':
      return `${Array.from({ length: 70 }, () => `<circle cx="${rand() * W}" cy="${rand() * 640}" r="${0.8 + rand() * 2.2}" fill="#fff" opacity="${0.3 + rand() * 0.6}"/>`).join('')}
        <path d="M300 160 L180 900 H420Z" fill="${a}" opacity="0.28"/>
        <line x1="300" y1="80" x2="300" y2="560" stroke="${a}" stroke-width="4"/>
        <path d="M300 420 L170 900 H430Z" fill="${g}"/>`
    case 'dunes':
      return `<circle cx="180" cy="300" r="70" fill="${a}"/>
        <path d="M0 560 C150 470 300 520 600 430 V900 H0Z" fill="${g}" opacity="0.6"/>
        <path d="M0 650 C200 560 380 640 600 560 V900 H0Z" fill="${g}" opacity="0.85"/>
        <path d="M0 740 C220 680 420 760 600 700 V900 H0Z" fill="${g}"/>`
    case 'pines': {
      const tree = (x: number, base: number, h: number, fill: string) =>
        `<path d="M${x} ${base - h} L${x - h * 0.28} ${base} H${x + h * 0.28}Z" fill="${fill}"/>`
      const back = Array.from({ length: 9 }, (_, i) => tree(i * 75 + rand() * 30, 640, 170 + rand() * 90, g)).join('')
      return `<circle cx="430" cy="230" r="58" fill="${a}" opacity="0.9"/>
        <g opacity="0.55">${back}</g>
        ${Array.from({ length: 7 }, (_, i) => tree(i * 100 + rand() * 40, 760, 260 + rand() * 120, g)).join('')}
        <rect x="0" y="755" width="${W}" height="145" fill="${g}"/>`
    }
    case 'ripples':
      return `${[60, 120, 180, 240, 300].map((r, i) => `<circle cx="300" cy="460" r="${r}" fill="none" stroke="${g}" stroke-width="${10 - i}" opacity="${0.9 - i * 0.14}"/>`).join('')}
        <path d="M250 430 C290 390 360 410 380 460 C350 480 300 500 250 470 L210 500 L220 450 Z" fill="${a}"/>
        <path d="M330 560 C360 530 410 540 420 575 C400 590 365 600 330 585 L300 605 L307 568 Z" fill="${a}" opacity="0.7"/>`
    case 'city': {
      const blocks: string[] = []
      for (let x = 0; x < W; ) {
        const w = 40 + rand() * 60
        const h = 160 + rand() * 300
        blocks.push(`<rect x="${x}" y="${760 - h}" width="${w - 6}" height="${h}" fill="${g}"/>`)
        for (let wy = 780 - h; wy < 740; wy += 28)
          if (rand() < 0.35) blocks.push(`<rect x="${x + 10}" y="${wy}" width="8" height="10" fill="${a}" opacity="0.8"/>`)
        x += w
      }
      return `<circle cx="160" cy="220" r="80" fill="${a}" opacity="0.85"/>${blocks.join('')}<rect x="0" y="755" width="${W}" height="145" fill="${g}"/>`
    }
    case 'rings':
      return `${Array.from({ length: 7 }, (_, i) => `<circle cx="300" cy="400" r="${40 + i * 38}" fill="none" stroke="${a}" stroke-opacity="${0.85 - i * 0.1}" stroke-width="${i === 0 ? 30 : 3}"/>`).join('')}`
  }
}

/** The cover's art without its title, which the page sets in its own type. */
export function coverSvg(cover: Cover): string {
  const rand = random(cover.slug)
  const id = cover.slug
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">
<defs>
<linearGradient id="${id}-sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${cover.sky[0]}"/><stop offset="1" stop-color="${cover.sky[1]}"/></linearGradient>
<linearGradient id="${id}-foot" x1="0" y1="0" x2="0" y2="1"><stop offset="0.55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.55"/></linearGradient>
</defs>
<rect width="${W}" height="${H}" fill="url(#${id}-sky)"/>
${motif(cover, rand)}
<rect width="${W}" height="${H}" fill="url(#${id}-foot)"/>
</svg>`
}
