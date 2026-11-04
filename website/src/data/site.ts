/** Facts the whole site shares. Every number here comes from the README or the docs. */

export const SITE = 'https://irori.app'
export const REPO = 'https://github.com/irori-app/irori'
export const REPO_API = 'https://api.github.com/repos/irori-app/irori'

/** Measured on Irori 0.2.0 on Linux with an empty profile and the window open (README, Performance). */
export const MEASURED = {
  memoryMiB: 88,
  idleCpu: 0,
  binaryMB: 76,
  languages: 19,
  heroicMiB: 478,
  heroicProcesses: 7,
}

/** Screenshots under `/screenshots`; titles and captions are in the dictionaries (`screens`). */
export const SCREENS = {
  library: { light: true },
  game: { light: true },
  'game-kiln': { light: false },
  downloads: { light: true },
  emulators: { light: true },
  stats: { light: true },
  'library-list': { light: false },
  'game-emulated': { light: false },
  wine: { light: true },
  bigpicture: { light: false },
  accounts: { light: false },
  'settings-appearance': { light: false },
} as const satisfies Record<string, { light: boolean }>

export type ScreenId = keyof typeof SCREENS

export const STORES = [
  { id: 'epic', name: 'Epic Games', feature: 'stores' },
  { id: 'gog', name: 'GOG', feature: 'stores' },
  { id: 'amazon', name: 'Amazon Games', feature: 'stores' },
  { id: 'zoom', name: 'ZOOM Platform', feature: 'stores' },
  { id: 'itch', name: 'itch.io', feature: 'stores' },
  { id: 'steam', name: 'Steam', feature: 'stores' },
  { id: 'battlenet', name: 'Battle.net', feature: 'stores' },
  { id: 'ea', name: 'EA app', feature: 'stores' },
  { id: 'ubisoft', name: 'Ubisoft Connect', feature: 'stores' },
  { id: 'rockstar', name: 'Rockstar Games Launcher', feature: 'stores' },
] as const

/** Systems as Irori badges them in the library. */
export const SYSTEMS = ['NES', 'SNES', 'N64', 'GB', 'GBC', 'GBA', 'NDS', '3DS', 'GC', 'Wii', 'PS1', 'PS2', 'PSP', 'Genesis', 'PCE', 'DOS', 'Arcade']

export type Platform = 'linux' | 'macos' | 'windows'

/** Install commands per system. Labels and notes are in the dictionaries (`download.options`). */
export const INSTALL: Record<Platform, { id: 'debian' | 'fedora' | 'flatpak' | 'arch' | 'appimage' | 'homebrew' | 'dmg' | 'installer' | 'portable' | 'scoop'; commands: string[] }[]> = {
  linux: [
    {
      id: 'debian',
      commands: [
        'sudo apt install -y curl',
        'curl -fsSL https://packages.irori.app/irori.gpg | sudo tee /usr/share/keyrings/irori.gpg > /dev/null',
        'curl -fsSL https://packages.irori.app/irori.list | sudo tee /etc/apt/sources.list.d/irori.list',
        'sudo apt update && sudo apt install irori',
      ],
    },
    {
      id: 'fedora',
      commands: [
        'sudo curl -fsSLo /etc/yum.repos.d/irori.repo https://packages.irori.app/irori.repo',
        'sudo dnf install irori',
      ],
    },
    {
      id: 'arch',
      commands: [
        'curl -fsSLo /tmp/irori.asc https://packages.irori.app/irori.asc',
        'sudo pacman-key --add /tmp/irori.asc && sudo pacman-key --lsign-key 6DD7B72703FE3BD5C719F4CE6FE681AB798DF656',
        "printf '\\n[irori]\\nServer = https://packages.irori.app/arch/$arch\\n' | sudo tee -a /etc/pacman.conf",
        'sudo pacman -Syu irori',
      ],
    },
    {
      id: 'flatpak',
      commands: [
        'flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo',
        'flatpak install --user ./Irori-*-linux.flatpak',
      ],
    },
    {
      id: 'appimage',
      commands: ['chmod +x Irori-*-x86_64.AppImage', './Irori-*-x86_64.AppImage'],
    },
  ],
  macos: [
    { id: 'homebrew', commands: ['brew install --cask irori-app/tap/irori'] },
    { id: 'dmg', commands: [] },
  ],
  windows: [
    { id: 'installer', commands: [] },
    { id: 'portable', commands: [] },
    { id: 'scoop', commands: ['scoop bucket add irori https://github.com/irori-app/scoop-bucket', 'scoop install irori'] },
  ],
}
