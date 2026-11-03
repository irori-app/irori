/** One Open Graph image per page, drawn at build time (`src/lib/og.ts`). */
import type { APIRoute, GetStaticPaths } from 'astro'
import { allPages, type PageInfo } from '../../lib/pages'
import { ogImage } from '../../lib/og'

export const getStaticPaths = (async () =>
  (await allPages('en')).map((page) => ({ params: { key: page.key }, props: { page } }))) satisfies GetStaticPaths

export const GET: APIRoute = async ({ props }) =>
  new Response(new Uint8Array(await ogImage((props as { page: PageInfo }).page)), { headers: { 'Content-Type': 'image/png' } })
