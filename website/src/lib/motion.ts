/**
 * The site's shared motion, run once per page: sections that rise into view,
 * numbers that count up, and the glow that follows the pointer over cards. All
 * of it stands still for visitors who ask for reduced motion.
 */

const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches

/** Adds `is-visible` to `[data-reveal]` and `[data-animate]` elements the first time they show. */
function reveal() {
  const targets = document.querySelectorAll<HTMLElement>('[data-reveal], [data-animate]')
  if (!('IntersectionObserver' in window) || reduced()) {
    targets.forEach((el) => el.classList.add('is-visible'))
    return
  }
  const observer = new IntersectionObserver(
    (seen) => {
      for (const entry of seen) {
        if (!entry.isIntersecting) continue
        entry.target.classList.add('is-visible')
        entry.target.dispatchEvent(new CustomEvent('irori:visible'))
        observer.unobserve(entry.target)
      }
    },
    { rootMargin: '0px 0px -8% 0px', threshold: 0.12 },
  )
  targets.forEach((el) => observer.observe(el))
}

/**
 * Counts `[data-count]` elements up to the number they already show. The page
 * carries the final number, so search engines and visitors without scripts see it.
 */
function counters() {
  const targets = document.querySelectorAll<HTMLElement>('[data-count]')
  if (reduced() || !('IntersectionObserver' in window)) return
  const run = (el: HTMLElement) => {
    const end = Number(el.dataset.count)
    const decimals = (el.dataset.count?.split('.')[1] ?? '').length
    const duration = 1400
    const start = performance.now()
    const step = (now: number) => {
      const p = Math.min(1, (now - start) / duration)
      const eased = 1 - Math.pow(1 - p, 4)
      el.textContent = (end * eased).toFixed(decimals)
      if (p < 1) requestAnimationFrame(step)
    }
    requestAnimationFrame(step)
  }
  const observer = new IntersectionObserver(
    (seen) => {
      for (const entry of seen) {
        if (!entry.isIntersecting) continue
        run(entry.target as HTMLElement)
        observer.unobserve(entry.target)
      }
    },
    { threshold: 0.6 },
  )
  targets.forEach((el) => observer.observe(el))
}

/** Feeds the pointer position to `[data-glow]` cards as `--x` and `--y`. */
function glow() {
  if (reduced() || !matchMedia('(hover: hover)').matches) return
  document.querySelectorAll<HTMLElement>('[data-glow]').forEach((card) => {
    card.addEventListener('pointermove', (event) => {
      const box = card.getBoundingClientRect()
      card.style.setProperty('--x', `${event.clientX - box.left}px`)
      card.style.setProperty('--y', `${event.clientY - box.top}px`)
    })
  })
}

export function startMotion() {
  reveal()
  counters()
  glow()
}
