FROM node:22-bookworm-slim AS build
WORKDIR /src
RUN npm install -g pnpm@12.14.0
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM caddy:2
COPY --from=build /src/dist /srv/web
