import { onMounted, onUnmounted, ref } from 'vue'

/**
 * Whether the page is dark. The theme switch (`ThemeSwitch.astro`) owns the
 * choice; Vue islands only follow the `dark` class on `<html>`.
 */
export function useTheme() {
  const isDark = ref(true)
  let observer: MutationObserver | undefined
  onMounted(() => {
    const root = document.documentElement
    isDark.value = root.classList.contains('dark')
    observer = new MutationObserver(() => (isDark.value = root.classList.contains('dark')))
    observer.observe(root, { attributes: true, attributeFilter: ['class'] })
  })
  onUnmounted(() => observer?.disconnect())
  return { isDark }
}
