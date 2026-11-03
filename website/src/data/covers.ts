/**
 * The made-up games of the demo library that the app's screenshots show, with covers
 * drawn as SVG by `/covers/<slug>.svg` (`src/pages/covers/[slug].svg.ts`). The stats card
 * uses them, and the cover walls fall back to them when the real game covers
 * (`game-covers.json`) aren't on disk.
 */

export type Motif = 'sun' | 'moon' | 'waves' | 'orbits' | 'grid' | 'mosaic' | 'beam' | 'dunes' | 'pines' | 'ripples' | 'city' | 'rings'

export interface Cover {
  slug: string
  title: string
  /** Sky from top to bottom, then the colour of the motif and of the ground. */
  sky: [string, string]
  motif: Motif
  accent: string
  ground: string
  /** The title's typeface: rounded (display), serif or plain. */
  face?: 'display' | 'serif' | 'plain'
  /** Replace the drawn art with an image under `public/`. */
  src?: string
  titleInArt?: boolean
}

export const COVERS: Cover[] = [
  { slug: 'saltglass-tide', title: 'Saltglass Tide', sky: ['#173246', '#2f6f86'], motif: 'waves', accent: '#7fa37a', ground: '#3d7e96' },
  { slug: 'moth-cartographer', title: 'The Moth Cartographer', sky: ['#211a3f', '#4b3487'], motif: 'orbits', accent: '#f1dca0', ground: '#3a2b6b', face: 'serif' },
  { slug: 'kiln-and-comet', title: 'Kiln & Comet', sky: ['#5a2516', '#9a3f22'], motif: 'sun', accent: '#f8cf8c', ground: '#b5502c', face: 'plain' },
  { slug: 'neon-kitsune-drift', title: 'Neon Kitsune Drift', sky: ['#240a36', '#a0246d'], motif: 'grid', accent: '#5fd0dc', ground: '#4c0f4a' },
  { slug: 'paper-orchard', title: 'Paper Orchard', sky: ['#e8ece2', '#d4dccb'], motif: 'mosaic', accent: '#d9735b', ground: '#7fa184', face: 'serif' },
  { slug: 'ironbark-hollow', title: 'Ironbark Hollow', sky: ['#1d3a2c', '#33604a'], motif: 'moon', accent: '#e7d48a', ground: '#25483a', face: 'serif' },
  { slug: 'signal-from-ima', title: 'Signal from Ima', sky: ['#060b1d', '#17224a'], motif: 'beam', accent: '#9fc1ff', ground: '#24356b' },
  { slug: 'ashfall-courier', title: 'Ashfall Courier', sky: ['#3a2a24', '#7a5a49'], motif: 'dunes', accent: '#f08a52', ground: '#5c4337' },
  { slug: 'glasshouse-rally', title: 'Glasshouse Rally', sky: ['#173a2a', '#2f7351'], motif: 'grid', accent: '#e6e7a6', ground: '#1f4a35' },
  { slug: 'koi-pond-quest', title: 'Koi Pond Quest', sky: ['#0e3b45', '#1d6c74'], motif: 'ripples', accent: '#ff8d5c', ground: '#145159', face: 'plain' },
  { slug: 'cinder-lanterns', title: 'Cinder Lanterns', sky: ['#1b0f0c', '#4b1c10'], motif: 'beam', accent: '#ffb24f', ground: '#6e2a14' },
  { slug: 'hollow-bell-pines', title: 'Hollow Bell Pines', sky: ['#c9d7d6', '#89a8a6'], motif: 'pines', accent: '#f2eee2', ground: '#2f4b46', face: 'serif' },
  { slug: 'tin-moon-city', title: 'Tin Moon City', sky: ['#14162b', '#3a2f63'], motif: 'city', accent: '#ffd36e', ground: '#0f1022' },
  { slug: 'dune-sparrow', title: 'Dune Sparrow', sky: ['#f6c08a', '#e3814e'], motif: 'dunes', accent: '#fff0c9', ground: '#b5532e' },
  { slug: 'quiet-orbit', title: 'Quiet Orbit', sky: ['#0b1220', '#1c2a44'], motif: 'rings', accent: '#ff6b2c', ground: '#1c2a44', face: 'plain' },
  { slug: 'lacquer-garden', title: 'Lacquer Garden', sky: ['#2a0d12', '#6e1b25'], motif: 'mosaic', accent: '#e9b44c', ground: '#3d1016', face: 'serif' },
  { slug: 'crystal-abacus', title: 'Crystal Abacus', sky: ['#3b2fa0', '#c9c4ea'], motif: 'rings', accent: '#ffffff', ground: '#5a4fc0', face: 'plain' },
  { slug: 'comet-festival', title: 'Comet Festival', sky: ['#f4f1ee', '#d6484c'], motif: 'orbits', accent: '#d6484c', ground: '#f4f1ee', face: 'plain' },
  { slug: 'brass-tortoise', title: 'Brass Tortoise', sky: ['#2e2a14', '#6d6224'], motif: 'sun', accent: '#ffe08a', ground: '#4a4318' },
  { slug: 'winter-relay', title: 'Winter Relay', sky: ['#dfe9f2', '#9fb7cf'], motif: 'pines', accent: '#ffffff', ground: '#4a6680' },
  { slug: 'ember-runner', title: 'Ember Runner', sky: ['#120a08', '#3d140a'], motif: 'grid', accent: '#ff6b2c', ground: '#2a0e07' },
  { slug: 'velvet-lighthouse', title: 'Velvet Lighthouse', sky: ['#1b1036', '#5a2a6e'], motif: 'beam', accent: '#ffe7a8', ground: '#2b1846', face: 'serif' },
  { slug: 'moss-and-mortar', title: 'Moss & Mortar', sky: ['#2b3a22', '#5f7a3e'], motif: 'moon', accent: '#f4e7b6', ground: '#33472a' },
  { slug: 'harbor-of-kites', title: 'Harbor of Kites', sky: ['#ffd9b8', '#f08f6d'], motif: 'waves', accent: '#2f6f86', ground: '#e86f55', face: 'serif' },
]
