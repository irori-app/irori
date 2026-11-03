/**
 * The comparison table on `/compare/` and, in part, on the home page. Each cell
 * is a verdict and a short note. Facts about other launchers come from their own
 * README or documentation; keep it that way, and date the check in the dictionary.
 */

export type Verdict = 'yes' | 'no' | 'partial' | 'info'
export interface Cell {
  v: Verdict
  note?: string
}

export const RIVALS = [
  { id: 'irori', name: 'Irori' },
  { id: 'heroic', name: 'Heroic' },
  { id: 'playnite', name: 'Playnite' },
  { id: 'lutris', name: 'Lutris' },
] as const

export type RivalId = (typeof RIVALS)[number]['id']

export interface Row {
  feature: string
  /** Shown in the home page's short table too. */
  home?: boolean
  cells: Record<RivalId, Cell>
}

export const ROWS: Row[] = [
  {
    feature: 'Linux',
    home: true,
    cells: { irori: { v: 'yes' }, heroic: { v: 'yes' }, playnite: { v: 'no' }, lutris: { v: 'yes' } },
  },
  {
    feature: 'macOS',
    home: true,
    cells: {
      irori: { v: 'yes', note: 'Wine, CrossOver, Game Porting Toolkit' },
      heroic: { v: 'yes', note: 'macOS 14 or newer' },
      playnite: { v: 'no' },
      lutris: { v: 'no' },
    },
  },
  {
    feature: 'Windows',
    home: true,
    cells: { irori: { v: 'yes' }, heroic: { v: 'yes' }, playnite: { v: 'yes' }, lutris: { v: 'no' } },
  },
  {
    feature: 'Built with',
    cells: {
      irori: { v: 'info', note: 'Rust and GPUI, native' },
      heroic: { v: 'info', note: 'Electron' },
      playnite: { v: 'info', note: 'C# and WPF' },
      lutris: { v: 'info', note: 'Python and GTK' },
    },
  },
  {
    feature: 'Epic Games and GOG downloads without helper tools',
    home: true,
    cells: {
      irori: { v: 'yes', note: 'Protocols built in, in Rust' },
      heroic: { v: 'no', note: 'Runs legendary and gogdl' },
      playnite: { v: 'no', note: 'Through the stores’ own clients' },
      lutris: { v: 'partial', note: 'GOG directly; Epic through its launcher in Wine' },
    },
  },
  {
    feature: 'Cloud saves',
    home: true,
    cells: {
      irori: { v: 'yes', note: 'Epic Games and GOG' },
      heroic: { v: 'yes', note: 'Epic Games and GOG' },
      playnite: { v: 'no', note: 'Left to the store clients' },
      lutris: { v: 'partial', note: 'GOG only' },
    },
  },
  {
    feature: 'itch.io',
    cells: { irori: { v: 'yes' }, heroic: { v: 'no' }, playnite: { v: 'yes' }, lutris: { v: 'yes' } },
  },
  {
    feature: 'Wine and Proton manager',
    cells: {
      irori: { v: 'yes', note: 'Linux and macOS' },
      heroic: { v: 'yes' },
      playnite: { v: 'no', note: 'Windows only' },
      lutris: { v: 'yes' },
    },
  },
  {
    feature: 'Emulators',
    home: true,
    cells: {
      irori: { v: 'yes', note: 'Installs and updates them' },
      heroic: { v: 'no' },
      playnite: { v: 'yes' },
      lutris: { v: 'yes', note: 'As runners' },
    },
  },
  {
    feature: 'Controller interface',
    cells: {
      irori: { v: 'yes', note: 'Big Picture' },
      heroic: { v: 'partial', note: 'Controller navigation' },
      playnite: { v: 'yes', note: 'Fullscreen mode' },
      lutris: { v: 'no' },
    },
  },
  {
    feature: 'Licence',
    home: true,
    cells: {
      irori: { v: 'info', note: 'MIT' },
      heroic: { v: 'info', note: 'GPL-3.0' },
      playnite: { v: 'info', note: 'MIT' },
      lutris: { v: 'info', note: 'GPL-3.0' },
    },
  },
]
