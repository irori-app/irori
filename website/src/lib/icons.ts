/** Lucide icons by the names content files use (`icon: wine`). */
import {
  BookOpen,
  ChartColumn,
  Download,
  FolderTree,
  Gamepad2,
  Gauge,
  Joystick,
  Keyboard,
  LifeBuoy,
  Library,
  Link,
  MessageCircleQuestion,
  Rocket,
  Store,
  Terminal,
  Tv,
  Wine,
} from '@lucide/vue'

export const ICONS = {
  library: Library,
  store: Store,
  gamepad: Gamepad2,
  wine: Wine,
  joystick: Joystick,
  tv: Tv,
  chart: ChartColumn,
  download: Download,
  gauge: Gauge,
  rocket: Rocket,
  book: BookOpen,
  keyboard: Keyboard,
  link: Link,
  terminal: Terminal,
  folder: FolderTree,
  help: LifeBuoy,
  faq: MessageCircleQuestion,
}

export function icon(name: string) {
  return ICONS[name as keyof typeof ICONS] ?? BookOpen
}
