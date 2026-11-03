import type { Platform } from '../data/site'

/** The visitor's system, guessed from the browser; Linux when it can't tell. */
export function detectPlatform(): Platform {
  const data = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData
  const name = data?.platform || navigator.userAgent
  if (/Windows/i.test(name)) return 'windows'
  if (/Mac|iPhone|iPad/i.test(name)) return 'macos'
  return 'linux'
}
