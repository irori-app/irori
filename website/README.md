# Irori website

The project's site at [irori.app](https://irori.app): static pages prerendered by
[Astro](https://astro.build), with Vue 3 for the few interactive parts (install tabs, the
screenshot gallery), Tailwind CSS 4 and shadcn-vue components (`src/components/ui`, copied
from the shadcn-vue registry and owned here). Every page is plain HTML with its own title,
description, canonical address, Open Graph image, breadcrumbs and structured data.

```bash
bun install
bun dev       # http://localhost:4321
bun run build # type-checks with astro check, then writes dist/
```

The build reads Irori's releases and star count from the GitHub API, once each. Without
network access it still succeeds: the changelog says the notes couldn't be loaded and the
star count is filled in by the visitor's browser. Set `GITHUB_TOKEN` to raise the API's
rate limit when building often (60 requests an hour without one).

## Layout

| Path | Holds |
| --- | --- |
| `src/pages/` | Routes. Each file only renders a view for one language |
| `src/views/` | The pages themselves, taking a `locale` |
| `src/layouts/Base.astro` | Head tags, navigation, footer and the motion script, on every page |
| `src/layouts/Article.astro` | Features, guides, comparisons and docs: breadcrumbs, table of contents, related pages |
| `src/content/<section>/<locale>/*.md` | The long-form pages, with their front matter checked by `src/content.config.ts` |
| `src/i18n/` | Languages (`config.ts`) and every interface string (`en.ts`) |
| `src/data/` | Facts the pages share: install commands, stores, measured numbers, the comparison table, covers and the tour's points |
| `src/lib/` | Build-time helpers: GitHub releases, Open Graph images, the cover drawings, the page list, motion |
| `src/pages/og/[...key].png.ts` | One 1200 × 630 Open Graph image per page, drawn with satori and resvg |
| `src/pages/covers/[slug].svg.ts` | The demo covers on the home page, drawn as SVG |

Facts on the site come from the repository's `README.md`, `AGENTS.md` and
`docs/PLATFORMS.md`. Keep them in step when Irori changes, and don't state anything the
app doesn't do. Comparisons with other launchers rely on those projects' own documentation;
update the date in `compare.checked` when you check them again.

## Adding a page

Add a Markdown file under `src/content/<section>/en/`; its route, sitemap entry, Open Graph
image and section index card follow from it. Descriptions are at most 165 characters.

## Adding a language

1. Add the locale to `LOCALES` in `src/i18n/config.ts` and to `i18n.locales` in
   `astro.config.mjs`, with its tags in `LANG_TAGS`.
2. Copy `src/i18n/en.ts` to `<locale>.ts`, translate it and register it in `DICTIONARIES`;
   its type is `Dictionary`, so a missing string fails the build.
3. Add `src/pages/[lang]/…` routes whose `getStaticPaths` lists the new locales and render
   the same views with `locale={lang}`, and translate the Markdown into
   `src/content/<section>/<locale>/`.
4. Translate `src/data/compare.ts`'s rows, the one table whose words live outside the
   dictionaries.

Internal links go through `localePath`, and every page already lists its translations with
`hreflang`.

## Motion

The hero's cover wall, the screenshot tour, counters, the stats card and section reveals are
CSS animations and a few small scripts (`src/lib/motion.ts` and each component's own
`<script>`), with no animation library. Everything stands still for visitors who ask for
reduced motion, and the cover wall has a visible pause button and stops when it scrolls off
screen. Content is in the HTML from the start, so nothing depends on a script to be read.

## Screenshots and covers

`public/screenshots/` holds the app's screens, as WebP at 1440 and 2880 pixels wide,
dark and (`light-` prefix) light. They come from the Linux release build running on a demo
library of real games with their store art, and invented play time and accounts, so no
personal data appears in them. Never replace them with captures of your own library or
real folder paths. `library.png`, `game.png`, `bigpicture.png` and `downloads.png` are PNG
copies of the 1440-pixel screens for Linux software centres, because AppStream expects PNG
or JPEG (`packaging/linux/*.metainfo.xml`); regenerate them when those screens change.

The cover walls (the home page's hero, and the bands on `/features/` and `/download/`) show
real game covers, to illustrate the games Irori runs; the footer credits their publishers.
The list is `src/data/game-covers.json` (title, Steam app id, file name). To change it, edit
the list and run:

```bash
bun covers              # fetch covers that are missing, and their accent colours
bun covers --force      # fetch them all again
```

`scripts/fetch-covers.mjs` downloads each cover once from Steam's public library art, cuts it
to 2:3 and stores AVIF and WebP at 300 and 600 pixels wide in `public/covers/games/`, which
is committed. The site serves these files itself and never loads anything from Steam. The
script fails, naming the game, when an app id has no image. Keep the list to games Irori can
run, and to cover art only.

If any listed cover is missing its files, the walls fall back to the demo library's drawn
covers (`src/data/covers.ts`, drawn by `src/pages/covers/[slug].svg.ts`), which the stats card
also uses. The tour's numbered points are percentages of the screenshot in
`src/data/hotspots.ts`; change `screen` and move the points to use another screenshot, and keep
their texts (`hotspots` in the dictionary) in the same order.

Fonts are self-hosted from Fontsource (Latin subsets only): Zen Maru Gothic for
headings, Zen Kaku Gothic New for text and M PLUS 1 Code for commands and figures.

## Deploying with Coolify

Create an application from this repository with the **Dockerfile** build pack, base
directory `/website`, port **80** and the domain **https://irori.app**. The image builds
the site and serves `dist/` with Caddy; Coolify's proxy adds TLS and the domain. Pages are
folders with an `index.html`, so Caddy needs no rewrites, and unknown addresses get
`404.html` with a 404 status. Add `GITHUB_TOKEN` as a build variable if builds hit GitHub's
rate limit.
