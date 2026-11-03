# Irori's website: built with Astro, served by Caddy on port 80.
FROM docker.io/library/node:24-alpine AS build
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
# Optional: raises GitHub's API rate limit for the changelog and star count.
ARG GITHUB_TOKEN
RUN pnpm build

FROM docker.io/library/caddy:2-alpine
COPY Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/dist /srv
EXPOSE 80
