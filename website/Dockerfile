# Irori's website: built with Astro, served by Caddy on port 80.
FROM docker.io/library/node:24-alpine AS build
# Bun installs the packages; Astro itself still runs on Node.
COPY --from=docker.io/oven/bun:1.4.4-alpine /usr/local/bin/bun /usr/local/bin/bun
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
# Optional: raises GitHub's API rate limit for the changelog and star count.
ARG GITHUB_TOKEN
RUN bun run build

FROM docker.io/library/caddy:2-alpine
COPY Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/dist /srv
EXPOSE 80
