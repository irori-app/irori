/** The demo covers as SVG files, drawn at build time from `src/data/covers.ts`. */
import type { APIRoute, GetStaticPaths } from 'astro'
import { COVERS, type Cover } from '../../data/covers'
import { coverSvg } from '../../lib/cover-svg'

export const getStaticPaths = (() =>
  COVERS.filter((cover) => !cover.src).map((cover) => ({ params: { slug: cover.slug }, props: { cover } }))) satisfies GetStaticPaths

export const GET: APIRoute = ({ props }) =>
  new Response(coverSvg((props as { cover: Cover }).cover), { headers: { 'Content-Type': 'image/svg+xml' } })
