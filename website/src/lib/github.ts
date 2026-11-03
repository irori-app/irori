/**
 * Releases and stars from GitHub, read once per build. Without network access the
 * site still builds: the changelog says the notes couldn't be loaded and the star
 * count waits for the visitor's browser to fetch it.
 */
import { marked } from 'marked'
import { REPO_API } from '../data/site'

export interface Release {
  tag: string
  version: string
  name: string
  date: string
  url: string
  html: string
}

const headers: Record<string, string> = {
  Accept: 'application/vnd.github+json',
  'User-Agent': 'irori.app build',
}
// A token only raises the rate limit; CI and Coolify builds work without one.
if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`

async function get<T>(path: string): Promise<T | null> {
  try {
    const response = await fetch(`${REPO_API}${path}`, { headers, signal: AbortSignal.timeout(10_000) })
    if (!response.ok) {
      console.warn(`[github] ${path}: HTTP ${response.status}`)
      return null
    }
    return (await response.json()) as T
  } catch (error) {
    console.warn(`[github] ${path}: ${error instanceof Error ? error.message : error}`)
    return null
  }
}

interface ApiRelease {
  tag_name: string
  name: string | null
  published_at: string | null
  html_url: string
  body: string | null
  draft: boolean
  prerelease: boolean
}

/** Shortens pull request addresses to `#123` and links `@name`, as GitHub shows them. */
function linkify(markdown: string): string {
  return markdown
    .replace(/https:\/\/github\.com\/irori-app\/irori\/pull\/(\d+)/g, '[#$1](https://github.com/irori-app/irori/pull/$1)')
    .replace(/(^|\s)@([a-zA-Z0-9-]+)/g, '$1[@$2](https://github.com/$2)')
}

let releases: Promise<Release[] | null> | undefined

export function getReleases(): Promise<Release[] | null> {
  releases ??= get<ApiRelease[]>('/releases?per_page=50').then((list) =>
    list
      ? list
          .filter((r) => !r.draft && !r.prerelease && /^v\d+\.\d+\.\d+$/.test(r.tag_name))
          .map((r) => ({
            tag: r.tag_name,
            version: r.tag_name.slice(1),
            name: r.name || r.tag_name,
            date: r.published_at ?? '',
            url: r.html_url,
            html: marked.parse(linkify(r.body ?? ''), { async: false }),
          }))
      : null,
  )
  return releases
}

export async function getLatestRelease(): Promise<Release | null> {
  return (await getReleases())?.[0] ?? null
}

let stars: Promise<number | null> | undefined

export function getStars(): Promise<number | null> {
  stars ??= get<{ stargazers_count: number }>('').then((repo) => repo?.stargazers_count ?? null)
  return stars
}
